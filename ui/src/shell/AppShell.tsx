/**
 * The MDReader application shell: overlay titlebar, collapsible sidebar, tab
 * strip, reading pane, related rail, status bar, and the keyboard-first overlays
 * (the Cmd+K search / Cmd+P file palette + shortcut sheet). It owns the wiring
 * the leaf components stay ignorant of: one KeymapRegistry driving both dispatch
 * and the sheet, session/settings persistence, the workspace watcher's
 * live-reload, provenance jumps, and theme application.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import {
  listWorkspaceFiles,
  listingFailureOf,
  onFsChanged,
  onWatchError,
  makeSearchAdapter,
  readDoc,
  saveDoc,
  startImport,
  type WorkspaceFile,
} from '../platform/fs.js';
import { SearchPalette } from '@mdreader/ui';
import type { HitTarget, PaletteFile } from '@mdreader/ui';
import { KeymapRegistry, detectMac } from '../keymap/registry.js';
import type { Binding } from '../keymap/registry.js';
import { selectActiveTab, useTabsStore } from '../stores/tabs.js';
import { useEditorStore } from '../stores/editor.js';
import type { EditorMode } from '../stores/editor.js';
import { classifyExternalChange, runSave } from './edit-save.js';
import { useSourcesStore } from '../stores/sources.js';
import {
  applyTheme,
  textScaleFactor,
  useSettingsStore,
} from '../stores/settings.js';
import { serializeSession } from '../stores/session.js';
import type { DocRef, ScrollPos } from '../stores/types.js';
import { getCurrentWindow } from '@tauri-apps/api/window';

import { isTauri, loadPersisted, saveSettings, sessionWriter } from './persistence.js';
import {
  grantLibrarySource,
  markLibrarySourceActive,
  pickAndAddLibrarySource,
  reconcileLibraryRestore,
  recordDiagnosticEventNonBlocking,
  settingsBackend,
  watchLibrarySource,
  type LibrarySourceDto,
} from '../platform/settings.js';
import type {
  EntityRow,
  LibraryEntityQueryResponse_Serialize,
  LibrarySourceExecution,
} from '../bindings.js';
import { events } from '../bindings.js';
import { createReindexer } from './reindex.js';
import { LoadingGate } from './loading-gate.js';
import { watcherStampCurrent } from './watcher-guard.js';
import { performSwitch } from './switch-workspace.js';
import { Titlebar } from './Titlebar.js';
import { Sidebar } from './Sidebar.js';
import { TabStrip } from './TabStrip.js';
import { ReaderPane } from './ReaderPane.js';
import type { JumpTarget } from './ReaderPane.js';
import { RelatedRail } from './RelatedRail.js';
import { StatusBar } from './StatusBar.js';
import { ShortcutSheet } from './ShortcutSheet.js';
import { SettingsSheet } from './SettingsSheet.js';
import { LibraryDocumentPane } from './LibraryDocumentPane.js';
import type { ShortcutGroup } from './ShortcutSheet.js';
import { UnsavedGuard } from './UnsavedGuard.js';
import { Toasts, toast } from './toasts.js';
import { hydrateLibrary, libraryHydrationMessage } from './library-documents.js';
import { makeSemanticsAdapter } from '../platform/semantics.js';
import type { ExplainedHit, SourceSpan, SpanSelection } from '../platform/semantics.js';
import { InspectorRail } from './lens/InspectorRail.js';
import { LensBand } from './lens/LensBand.js';
import { LensSheet } from './lens/LensSheet.js';
import { ConstraintSheet } from './lens/ConstraintSheet.js';
import { ExplainSheet } from './lens/ExplainSheet.js';
import { useSchemas } from './lens/use-schemas.js';
import { resolveSourcePath, spanFromSelection } from './lens/source-span.js';
import './shell.css';
import './lens/lens.css';

function isEditable(element: Element | null): boolean {
  if (!element) return false;
  const tag = element.tagName;
  return (
    tag === 'INPUT' ||
    tag === 'TEXTAREA' ||
    tag === 'SELECT' ||
    (element as HTMLElement).isContentEditable
  );
}

export function AppShell() {
  const isMac = useMemo(detectMac, []);

  // --- store slices -----------------------------------------------------
  const tabs = useTabsStore((state) => state.tabs);
  const activeTabId = useTabsStore((state) => state.activeTabId);
  const activeTab = useTabsStore(selectActiveTab);

  const root = useSourcesStore((state) => state.root);
  const files = useSourcesStore((state) => state.files);
  const recent = useSourcesStore((state) => state.recent);
  const watcher = useSourcesStore((state) => state.watcher);
  const loading = useSourcesStore((state) => state.loading);
  const importState = useSourcesStore((state) => state.importState);
  const fromIndex = useSourcesStore((state) => state.fromIndex);
  const listingError = useSourcesStore((state) => state.listingError);

  const theme = useSettingsStore((state) => state.theme);
  const readingFont = useSettingsStore((state) => state.readingFont);
  const textScaleIndex = useSettingsStore((state) => state.textScaleIndex);
  const sidebarCollapsed = useSettingsStore((state) => state.sidebarCollapsed);
  const outlineVisible = useSettingsStore((state) => state.outlineVisible);
  const relatedRailOpen = useSettingsStore((state) => state.relatedRailOpen);
  const inspectorOpen = useSettingsStore((state) => state.inspectorOpen);
  const lensSchema = useSettingsStore((state) => state.lensSchema);
  const reviewer = useSettingsStore((state) => state.reviewer);
  const focusMode = useSettingsStore((state) => state.focusMode);

  // --- local UI state ---------------------------------------------------
  const [paletteOpen, setPaletteOpen] = useState(false);
  // Which mode the palette opens in: Cmd+K search vs Cmd+P file jump (one
  // component, two modes — the P4 unification).
  const [paletteMode, setPaletteMode] = useState<'search' | 'file'>('file');
  const [sheetOpen, setSheetOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void events.mdreaderShowSettings
      .listen(() => setSettingsOpen(true))
      .then((off) => {
        if (disposed) off();
        else unlisten = off;
      })
      .catch(() => {
        /* not a live Tauri event surface */
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  const [libraryDocument, setLibraryDocument] = useState<{
    member: LibrarySourceExecution;
    row: EntityRow;
    evidence: LibraryEntityQueryResponse_Serialize;
  } | null>(null);
  const [libraryEvidence, setLibraryEvidence] =
    useState<LibraryEntityQueryResponse_Serialize[]>([]);
  const [librarySearch, setLibrarySearch] = useState('');
  const [libraryLoading, setLibraryLoading] = useState(false);
  const [libraryError, setLibraryError] = useState<string | null>(null);
  const libraryRefreshGeneration = useRef(0);
  const libraryRefreshRequest = useRef<string | null>(null);
  const clearLibraryDocument = useCallback(() => {
    setLibraryDocument(null);
  }, []);

  useEffect(() => {
    clearLibraryDocument();
  }, [activeTabId, clearLibraryDocument]);
  const [reloadNonce, setReloadNonce] = useState(0);
  const [docWords, setDocWords] = useState<number | null>(null);
  const [hydrated, setHydrated] = useState(false);
  // The unsaved-changes guard: closing a dirty tab, or quitting with dirty tabs,
  // routes through this prompt before anything is discarded.
  const [closeGuard, setCloseGuard] = useState<
    | { kind: 'tab'; tabId: string }
    | { kind: 'quit' }
    | {
        kind: 'switch';
        folder: string;
        sourceId?: string;
        expectedSourceIntent: number;
        resume?: {
          kind: 'library-edit';
          member: LibrarySourceExecution;
          row: EntityRow;
        };
      }
    | null
  >(null);
  // A pending provenance jump handed to the ReaderPane once the target renders.
  // It carries the WHOLE range, not just where to land: a citation that names
  // lines 41–47 must arrive at the reader still naming 41–47, or the reader
  // cannot mark what was actually cited. See `JumpTarget`.
  const [jump, setJump] = useState<JumpTarget | null>(null);
  const jumpSeq = useRef(0);

  // --- the semantic surfaces -------------------------------------------
  // Which corpus-scoped sheet is up (at most one; they are all Escape-closable).
  const [lensSheetOpen, setLensSheetOpen] = useState(false);
  const [constraintsOpen, setConstraintsOpen] = useState(false);
  const [explainOpen, setExplainOpen] = useState(false);
  // The passage the Inspector is reading claims for: a real, non-collapsed
  // selection in the reading column. It STICKS — clicking Approve collapses the
  // selection, and the panel must not empty itself out from under the ruling.
  const [inspected, setInspected] = useState<SpanSelection | null>(null);
  const [semanticWorkspaceGeneration, setSemanticWorkspaceGeneration] = useState(0);

  const refreshLibrary = useCallback(async () => {
    const generation = libraryRefreshGeneration.current + 1;
    libraryRefreshGeneration.current = generation;
    if (settingsBackend.cancelCloudDocumentQuery && libraryRefreshRequest.current) {
      await settingsBackend.cancelCloudDocumentQuery(libraryRefreshRequest.current);
    }
    setLibraryLoading(true);
    setLibraryError(null);
    try {
      const requestId = `library:${globalThis.crypto.randomUUID()}`;
      libraryRefreshRequest.current = requestId;
      const hydration = await hydrateLibrary(
        settingsBackend,
        requestId,
      );
      const { evidence: answers } = hydration;
      if (libraryRefreshGeneration.current !== generation) return;
      setLibraryEvidence(answers);
      setLibraryDocument((current) => {
        if (!current) return null;
        for (const evidence of answers) {
          const member = evidence.memberships.find(
            (candidate) => candidate.membership_id === current.member.membership_id,
          );
          const row = evidence.answer.rows.find(
            (candidate) => candidate.record_key === current.row.record_key,
          );
          if (member && row) return { member, row, evidence };
        }
        return null;
      });
      setLibraryError(libraryHydrationMessage(hydration));
    } catch {
      if (libraryRefreshGeneration.current === generation) {
        setLibraryEvidence([]);
        setLibraryDocument(null);
        setLibraryError('Library sources could not be read.');
      }
    } finally {
      if (libraryRefreshGeneration.current === generation) {
        libraryRefreshRequest.current = null;
        setLibraryLoading(false);
      }
    }
  }, []);

  useEffect(() => {
    if (!hydrated || !isTauri()) return;
    void refreshLibrary();
    return () => {
      libraryRefreshGeneration.current += 1;
      if (settingsBackend.cancelCloudDocumentQuery) {
        if (libraryRefreshRequest.current) {
          void settingsBackend.cancelCloudDocumentQuery(libraryRefreshRequest.current);
        }
      }
      libraryRefreshRequest.current = null;
    };
  // Persisted UI state can hydrate before the restored workspace has committed
  // its backend grant. Refresh again when that grant advances the semantic
  // generation so a startup read that raced the engine cannot leave the
  // Library empty until the user changes Settings or presses Refresh.
  }, [hydrated, refreshLibrary, semanticWorkspaceGeneration]);

  const activePathRef = useRef<string | null>(null);
  activePathRef.current = activeTab?.docRef.path ?? null;
  const activeTabIdRef = useRef<string | null>(null);
  activeTabIdRef.current = activeTabId;
  // Late-bound so the close-guard's `performQuit` (declared before `flushSession`)
  // can reach the latest flusher without a declaration-order dependency.
  const flushSessionRef = useRef<() => void>(() => {});
  // Late-bound so `proceedAfterGuard` (declared before `commitSwitch`) can reach it.
  const commitSwitchRef = useRef<
    (folder: string, sourceId?: string, expectedSourceIntent?: number) => Promise<boolean>
  >(() => Promise.resolve(false));
  const editLibraryDocumentRef = useRef<
    (
      member: LibrarySourceExecution,
      row: EntityRow,
      expectedSourceIntent?: number,
    ) => Promise<void>
  >(() => Promise.resolve());
  // Native watch/grant commands commit the backend epoch before their IPC
  // promise resolves. Serialize the complete frontend transaction so a newer
  // request cannot supersede an already-committed response and then fail while
  // leaving the UI on a different root from the backend.
  const switchQueueRef = useRef<Promise<void>>(Promise.resolve());
  // Every user operation that can activate, add, or forget a library source
  // advances this generation synchronously, before its first await. Startup
  // captures the earlier generation before loading persistence and may publish
  // only while it is still current.
  const sourceIntentRef = useRef(0);
  const beginSourceIntent = useCallback(() => {
    sourceIntentRef.current += 1;
    return sourceIntentRef.current;
  }, []);
  // Latched once the window is committing to close, so the force-close runs at
  // most once. We force-close with destroy(), which tears the window down WITHOUT
  // re-firing onCloseRequested — but the flag stays as a cheap guard against a
  // second close request racing in before destroy() completes. Per this window
  // instance (a ref, not module-level, so tests/mounts stay isolated).
  const closingRef = useRef(false);

  // --- workspace loading ------------------------------------------------
  // Frontend request generation, the UI-side mirror of the backend epoch: every
  // `openRoot` bumps it, and every async continuation (listing, import) captures
  // the value at dispatch and writes to the store ONLY while it is still current.
  // So a late continuation for an older root can never overwrite newer state.
  const openEpochRef = useRef(0);
  // The backend generation the CURRENT watcher was armed under (from
  // watchLibrarySource). Watcher events/errors carry the generation they were armed
  // under; we discard mismatches so a previous watcher's still-draining events
  // can't drive — or fail — the new workspace. `-1` = no watcher yet.
  const watcherGenRef = useRef(-1);
  // Backend library identity for the CURRENT committed root. It is assigned only
  // after the existing watch/grant transaction succeeds; diagnostics never mint
  // or substitute filesystem authority.
  const activeSourceIdRef = useRef<string | null>(null);
  // Generation-aware guard for the `loading` flag: a stale listing can't clear a
  // newer open's loading, and the newest open's failure always clears it.
  const loadingGateRef = useRef(new LoadingGate());
  const loadingSink = useMemo(
    () => ({ setLoading: (value: boolean) => useSourcesStore.getState().setLoading(value) }),
    [],
  );

  const refreshFiles = useCallback(async (folder: string, epoch: number) => {
    try {
      const listing = await listWorkspaceFiles(folder);
      if (openEpochRef.current !== epoch) return; // superseded by a newer open
      const sources = useSourcesStore.getState();
      sources.setFiles(listing.files);
      sources.setFromIndex(listing.fromIndex);
      sources.setListingError(null);
    } catch (cause) {
      if (openEpochRef.current !== epoch) return;
      // A refresh that could not read the folder must SAY so. Leaving the old
      // list up would present a stale tree as the live one; silently dropping
      // it would present a denial as an empty folder. Name the failure instead.
      const failure = listingFailureOf(cause);
      if (failure) useSourcesStore.getState().setListingError(failure);
      /* not a backend failure (e.g. no Tauri) — leave the list as-is */
    }
  }, []);

  // List the workspace's markdown files into the store, toggling `loading`. The
  // listing carries whether it came from the engine index or the live fs scan.
  const listInto = useCallback(
    async (folder: string, epoch: number) => {
      const sources = useSourcesStore.getState();
      loadingGateRef.current.begin(epoch, loadingSink);
      try {
        const listing = await listWorkspaceFiles(folder);
        if (openEpochRef.current === epoch) {
          sources.setFiles(listing.files);
          sources.setFromIndex(listing.fromIndex);
          sources.setListingError(null);
        }
      } catch (cause) {
        if (openEpochRef.current === epoch) {
          // THE bug this guards: clearing `files` and stopping here rendered a
          // folder the OS refused to read as "No markdown files in this folder
          // yet." — a false statement about the user's disk. The list is still
          // cleared (we have no listing), but the REASON travels with it so the
          // sidebar and status bar can say the folder is unreadable and why.
          sources.setFiles([]);
          sources.setFromIndex(false);
          sources.setListingError(
            listingFailureOf(cause) ?? {
              kind: 'failed',
              message: cause instanceof Error ? cause.message : String(cause),
            },
          );
        }
      } finally {
        // Generation-aware clear: a stale listing can't clear a newer open's
        // loading, but a newer-or-equal completion always does.
        loadingGateRef.current.complete(epoch, loadingSink);
      }
    },
    [loadingSink],
  );

  // Run an incremental import for `folder`, streaming coarse progress into the
  // store and resolving when it settles. Continuations write only while their
  // capturing `epoch` is still current (the backend likewise discards them per
  // its own epoch). Does NOT re-list: `triggerReindex` owns the import→refresh
  // ordering. Tauri-only (no backend engine on the website or in unit tests).
  const runImport = useCallback(async (folder: string, epoch: number): Promise<void> => {
    if (!isTauri() || openEpochRef.current !== epoch) return;
    const sources = useSourcesStore.getState();
    sources.setImport({ phase: 'importing', filesDone: 0, filesTotal: 0, chunks: 0, error: null });
    const recordIndex = (state: {
      phase: 'idle' | 'loading' | 'importing' | 'reembedding' | 'done' | 'model-unavailable' | 'error';
      filesDone: number;
      filesTotal: number;
      chunks: number;
    }) => {
      const sourceId = activeSourceIdRef.current;
      if (!sourceId) return;
      recordDiagnosticEventNonBlocking({
        kind: 'index',
        source_id: sourceId,
        phase: state.phase === 'idle' ? 'importing' : state.phase,
        files_done: state.filesDone,
        files_total: state.filesTotal,
        chunks: state.chunks,
        error:
          state.phase === 'error' || state.phase === 'model-unavailable'
            ? 'index-unavailable'
            : null,
      });
    };
    recordIndex({
      phase: 'importing',
      filesDone: 0,
      filesTotal: 0,
      chunks: 0,
    });
    try {
      const final = await startImport(folder, (progress) => {
        if (openEpochRef.current === epoch) {
          sources.setImport(progress);
          recordIndex(progress);
        }
      });
      if (openEpochRef.current !== epoch) return;
      sources.setImport(final);
      recordIndex(final);
    } catch (cause) {
      if (openEpochRef.current !== epoch) return;
      const failed = {
        phase: 'error',
        filesDone: 0,
        filesTotal: 0,
        chunks: 0,
        error: cause instanceof Error ? cause.message : String(cause),
      } as const;
      sources.setImport(failed);
      recordIndex(failed);
    }
  }, []);

  // The single reindex coordinator: a fresh open, a settled fs change, or a save
  // runs an incremental import THEN refreshes the listing; a change that lands
  // while an import is already in flight queues exactly one follow-up reindex so
  // it is never dropped from the index (see reindex.ts). Reads the CURRENT root +
  // request generation at each step (`runImport`/`refreshFiles` self-guard on the
  // epoch), so a switch mid-flight is handled by those guards.
  const reindexer = useMemo(
    () =>
      createReindexer({
        startImport: async () => {
          const folder = useSourcesStore.getState().root;
          if (folder) await runImport(folder, openEpochRef.current);
        },
        refreshFiles: async () => {
          const folder = useSourcesStore.getState().root;
          if (folder) await refreshFiles(folder, openEpochRef.current);
        },
      }),
    [runImport, refreshFiles],
  );
  const triggerReindex = useCallback(() => {
    if (!isTauri()) return Promise.resolve('queued' as const);
    return reindexer.onChange();
  }, [reindexer]);

  // Open `folder` as the active workspace; resolves to `true` once the store
  // reflects it as the active root. In a Tauri shell, only an opaque library
  // source ID can arm the watcher and establish the backend grant. The
  // watch-and-grant transaction is atomic, so a switch that fails to arm leaves
  // the previous grant and watcher intact. A first open (or same-root reopen)
  // whose watch fails may use the backend's source-ID degraded grant, without
  // live reload. Never closes tabs — a workspace switch is the caller's concern.
  const openRoot = useCallback(
    async (folder: string, sourceId?: string): Promise<boolean> => {
      const sources = useSourcesStore.getState();
      // This open supersedes any prior in-flight open — INCLUDING a re-open of the
      // SAME root (e.g. a session-restore open racing a user pick): bump the
      // request generation and thread it through every continuation below, so the
      // older open's late listing/import writes are all discarded. `previousRoot`
      // is snapshotted HERE, synchronously before any await, so two overlapping
      // switches each capture the true prior root (the first's `setRoot` only runs
      // after its watch await, i.e. strictly after this snapshot) — the root the
      // retire step must attribute the retired tabs to.
      const epoch = (openEpochRef.current += 1);
      const previousRoot = sources.root;
      const isSwitch = previousRoot !== null && previousRoot !== folder;

      // Website / tests without a Tauri backend: no grant/watch IPC. Open the
      // folder best-effort so the shell still renders content.
      if (!isTauri()) {
        sources.setRoot(folder);
        setSemanticWorkspaceGeneration(epoch);
        sources.setWatcher('idle');
        await listInto(folder, epoch);
        // A newer open superseded this one during listInto → report NOT-current so
        // its (stale) success can't authorize retiring the newer workspace's tabs.
        if (openEpochRef.current !== epoch) return false;
        activeSourceIdRef.current = sourceId ?? null;
        return true;
      }

      if (!sourceId) {
        loadingGateRef.current.fail(epoch, loadingSink);
        if (openEpochRef.current === epoch) {
          toast('This folder must be admitted through the library before it can be opened');
        }
        return false;
      }

      let grant: { root: string; generation: number };
      try {
        // The ONE atomic switch: arms the watcher, adopts `folder` as the grant
        // (advancing the epoch), and returns the canonical root plus the
        // generation the watcher was armed under.
        // A registry member is activated by opaque source ID, letting the
        // backend carry its native PathBuf straight into the watcher/epoch
        // transaction. `folder` remains display text only.
        grant = await watchLibrarySource(sourceId);
      } catch {
        if (isSwitch) {
          // A real switch that could not arm: stay entirely on the old workspace.
          // The backend kept the OLD watcher live, so its status is unchanged and
          // still accurate — do NOT flip it to 'error' (that would slander a
          // workspace that is in fact healthily watched). Surface the switch
          // failure transiently instead. Its failure clears a stuck loading only
          // if this open is the newest (generation >= the one that set loading);
          // an older straggler cannot clear a newer open's loading.
          loadingGateRef.current.fail(epoch, loadingSink);
          // Suppress a STALE failure toast: if a newer open already superseded
          // this one, its failure is moot (we've moved on to another workspace) —
          // toasting would nag about a switch the user has already left behind.
          if (openEpochRef.current === epoch) {
            toast('Could not switch workspace — staying on the current folder');
          }
          return false;
        }
        // First open / same-root reopen: grant independently, read degraded.
        let g: string;
        try {
          g = await grantLibrarySource(sourceId);
        } catch {
          // A registry member that can no longer be resolved must remain closed;
          // never turn its lossy DTO path into an independent grant.
          loadingGateRef.current.fail(epoch, loadingSink);
          if (openEpochRef.current === epoch) {
            toast('Could not open this library source');
          }
          return false;
        }
        // A newer open superseded this one during the await → abandon its store
        // mutations (the newer open owns the workspace now).
        if (openEpochRef.current !== epoch) return false;
        // The backend grant is committed now. Publish its identity before any
        // awaited listing so a runtime error from this source cannot be
        // attributed to the previous source while the new listing is pending.
        activeSourceIdRef.current = sourceId ?? null;
        sources.setRoot(g);
        setSemanticWorkspaceGeneration(epoch);
        sources.setWatcher('error');
        sources.setImport(null); // fresh workspace: no import belongs to it yet
        await listInto(g, epoch);
        // A newer open superseded this one during listInto → report NOT-current
        // so the caller does not retire the newer workspace's tabs.
        if (openEpochRef.current !== epoch) return false;
        void triggerReindex();
        return true;
      }

      // A newer open superseded this one during the watch await → abandon.
      if (openEpochRef.current !== epoch) return false;
      // Watch armed + grant committed atomically; adopt the generation as the
      // reference for filtering this watcher's events.
      watcherGenRef.current = grant.generation;
      // The new backend epoch is authoritative now. Publish its source identity
      // before any awaited listing so this watcher's errors and reindex progress
      // can never inherit the previous source ID.
      activeSourceIdRef.current = sourceId ?? null;
      sources.setRoot(grant.root);
      setSemanticWorkspaceGeneration(grant.generation);
      sources.setWatcher('watching');
      sources.setImport(null); // fresh workspace: no import belongs to it yet
      await listInto(grant.root, epoch);
      // A newer open superseded this one during listInto → report NOT-current so
      // its (stale) success cannot authorize closeAll on the newer workspace.
      if (openEpochRef.current !== epoch) return false;
      void triggerReindex();
      return true;
    },
    [listInto, triggerReindex, loadingSink],
  );

  // Commit a workspace switch: retire the previous workspace's open tabs into
  // (their own) Recent, but only once the new root is actually live. Retiring a
  // tab tears it down, so this must run only after any unsaved edits have been
  // resolved — the guard in `switchWorkspace` guarantees that.
  const commitSwitch = useCallback(
    (
      folder: string,
      sourceId?: string,
      expectedSourceIntent?: number,
    ): Promise<boolean> => {
      const run = async (): Promise<boolean> => {
        if (
          expectedSourceIntent !== undefined &&
          sourceIntentRef.current !== expectedSourceIntent
        ) {
          return false;
        }
        const sources = useSourcesStore.getState();
        // `performSwitch` owns the two guarantees (same-root supersede; per-origin
        // retirement against the post-commit root) — see switch-workspace.ts.
        const committed = await performSwitch(folder, {
          openRoot: (target) => openRoot(target, sourceId),
          previousRoot: sources.root,
          currentRoot: () => useSourcesStore.getState().root,
          retireForeign: (root) => useTabsStore.getState().retireForeign(root),
          addRecent: (ref, root) => sources.addRecent(ref, root),
        });
        if (!committed) return false;

        // Membership never grants access. The source-ID watch/grant transaction
        // above committed the real backend epoch first; only now may the registry
        // remember which already-admitted source should restore next launch.
        if (sourceId) {
          try {
            await markLibrarySourceActive(sourceId);
          } catch {
            if (useSourcesStore.getState().root === folder) {
              toast('Workspace opened, but its library restore preference could not be saved');
            }
            return useSourcesStore.getState().root === folder;
          }
          activeSourceIdRef.current = sourceId;
          recordDiagnosticEventNonBlocking({
            kind: 'workspace',
            source_id: sourceId,
            active: true,
          });
          recordDiagnosticEventNonBlocking({
            kind: 'watcher',
            source_id: sourceId,
            state: useSourcesStore.getState().watcher === 'watching' ? 'watching' : 'error',
            error:
              useSourcesStore.getState().watcher === 'watching' ? null : 'watcher-unavailable',
          });
        } else {
          activeSourceIdRef.current = null;
        }
        return true;
      };

      const result = switchQueueRef.current.then(run, run);
      switchQueueRef.current = result.then(
        () => undefined,
        () => undefined,
      );
      return result;
    },
    [openRoot],
  );
  commitSwitchRef.current = commitSwitch;

  // Switch the active workspace to `folder`. A real switch (a different root)
  // retires the previous workspace's tabs, which would tear down any unsaved edit
  // — so if ANY tab is dirty, route through the unsaved-changes guard first
  // (Save-all / Discard / Cancel) and let it commit the switch. A clean workspace
  // (or a same-root supersede) switches immediately.
  const switchWorkspace = useCallback(
    (
      folder: string,
      sourceId?: string,
      resume?: {
        kind: 'library-edit';
        member: LibrarySourceExecution;
        row: EntityRow;
      },
      existingSourceIntent?: number,
    ): Promise<boolean> => {
      const expectedSourceIntent = existingSourceIntent ?? beginSourceIntent();
      if (sourceIntentRef.current !== expectedSourceIntent) {
        return Promise.resolve(false);
      }
      const currentRoot = useSourcesStore.getState().root;
      const isRealSwitch = currentRoot !== null && currentRoot !== folder;
      if (isRealSwitch && useEditorStore.getState().dirtyTabIds().length > 0) {
        setCloseGuard({ kind: 'switch', folder, sourceId, expectedSourceIntent, resume });
        return Promise.resolve(false);
      }
      return commitSwitch(folder, sourceId, expectedSourceIntent);
    },
    [beginSourceIntent, commitSwitch],
  );

  const onPickFolder = useCallback(async () => {
    const expectedSourceIntent = beginSourceIntent();
    let picked: { source: LibrarySourceDto | null };
    try {
      picked = await pickAndAddLibrarySource();
    } catch {
      toast('Could not add this folder to the library');
      return;
    }
    if (sourceIntentRef.current !== expectedSourceIntent) return;
    if (!picked.source) return;
    await switchWorkspace(picked.source.path, picked.source.id, undefined, expectedSourceIntent);
  }, [beginSourceIntent, switchWorkspace]);

  // Re-run the import for the current workspace. When the failure was the model
  // (the `model-unavailable` phase), this genuinely re-attempts the LOAD: the
  // backend's index is already open, and `start_import` re-enters its model step
  // — which is the step that failed — rather than merely re-scanning files.
  // Rides the current open generation.
  const retryIndex = useCallback(() => {
    void triggerReindex();
  }, [triggerReindex]);

  // --- open / navigate helpers -----------------------------------------
  const openRef = useCallback((ref: DocRef) => {
    clearLibraryDocument();
    const root = useSourcesStore.getState().root;
    useTabsStore.getState().openTab(ref, { originRoot: root });
    useSourcesStore.getState().addRecent(ref, root);
  }, [clearLibraryDocument]);

  const openFile = useCallback(
    (file: WorkspaceFile) => {
      openRef({ path: file.path, name: file.name });
      setPaletteOpen(false);
    },
    [openRef],
  );

  // Switch the active tab's view mode for its CURRENT document. Entering
  // source/split for the first time seeds the editor buffer from a fresh disk read
  // (content + hash) so the save guard is anchored to exactly what was on disk
  // when editing began. Keyed by (tab, doc), so a doc the tab navigated away from
  // keeps its own entry untouched.
  const setEditorMode = useCallback(async (mode: EditorMode, expectedSourceIntent?: number) => {
    const sourceIntentIsCurrent = () =>
      expectedSourceIntent === undefined || sourceIntentRef.current === expectedSourceIntent;
    if (!sourceIntentIsCurrent()) return;
    const tab = selectActiveTab(useTabsStore.getState());
    if (!tab) return;
    const tabId = tab.id;
    const docPath = tab.docRef.path;
    const editor = useEditorStore.getState();
    if (mode === 'read') {
      if (editor.entry(tabId, docPath)) editor.setMode(tabId, docPath, 'read');
      return;
    }
    if (editor.entry(tabId, docPath)) {
      editor.setMode(tabId, docPath, mode);
      return;
    }
    try {
      const doc = await readDoc(docPath);
      if (!sourceIntentIsCurrent()) return;
      // The active tab may have changed (or navigated) during the read — seed only
      // if it still stands on the same document.
      const landed = selectActiveTab(useTabsStore.getState());
      if (!landed || landed.id !== tabId || landed.docRef.path !== docPath) return;
      useEditorStore.getState().seed(tabId, docPath, doc.content, doc.hash, mode);
    } catch (cause) {
      if (sourceIntentIsCurrent()) {
        toast(`Could not open for editing — ${cause instanceof Error ? cause.message : String(cause)}`);
      }
    }
  }, []);

  const editLibraryDocument = useCallback(
    async (
      member: LibrarySourceExecution,
      row: EntityRow,
      expectedSourceIntent?: number,
    ): Promise<void> => {
      const editSourceIntent = expectedSourceIntent ?? beginSourceIntent();
      const sourceIntentIsCurrent = () => sourceIntentRef.current === editSourceIntent;
      if (!sourceIntentIsCurrent()) return;
      if (!member.editable) return;
      const path = typeof row.fields.path === 'string' ? row.fields.path : null;
      if (!path) {
        toast('This Library result has no editable local path');
        return;
      }
      let snapshot;
      try {
        snapshot = await settingsBackend.listSources();
      } catch {
        if (sourceIntentIsCurrent()) toast('Could not resolve this Library source');
        return;
      }
      if (!sourceIntentIsCurrent()) return;
      const source = snapshot.sources.find((candidate) => candidate.id === member.registry_id);
      if (!source) {
        toast('This Library source is no longer registered');
        return;
      }
      if (useSourcesStore.getState().root !== source.path) {
        const switched = await switchWorkspace(source.path, source.id, {
          kind: 'library-edit',
          member,
          row,
        }, editSourceIntent);
        if (!switched) return;
      }
      if (!sourceIntentIsCurrent()) return;
      if (!settingsBackend.resolveLibraryDocumentEdit) {
        toast('Local Library editing is unavailable');
        return;
      }
      let documentPath;
      try {
        documentPath = await settingsBackend.resolveLibraryDocumentEdit(source.id, path);
      } catch {
        if (sourceIntentIsCurrent()) {
          toast('This Library document is no longer available for editing');
        }
        return;
      }
      if (!sourceIntentIsCurrent()) return;
      const title =
        typeof row.fields.title === 'string' && row.fields.title.length > 0
          ? row.fields.title
          : path.split(/[\\/]+/).pop() ?? path;
      openRef({ path: documentPath, name: title });
      if (!sourceIntentIsCurrent()) return;
      await setEditorMode('source', editSourceIntent);
    },
    [beginSourceIntent, openRef, setEditorMode, switchWorkspace],
  );
  editLibraryDocumentRef.current = editLibraryDocument;

  // The palette's file open (Cmd+P mode): foreground focuses the tab; ⌘↵ opens
  // it in the background (the palette itself decides when to close).
  const onOpenPaletteFile = useCallback((file: PaletteFile, opts: { background: boolean }) => {
    if (!opts.background) clearLibraryDocument();
    const root = useSourcesStore.getState().root;
    const ref = { path: file.path, name: file.name };
    useTabsStore.getState().openTab(ref, { activate: !opts.background, originRoot: root });
    useSourcesStore.getState().addRecent(ref, root);
  }, [clearLibraryDocument]);

  // The palette's search-hit open: land at the hit's provenance line and mark the
  // chunk that matched. Enter focuses the tab and schedules the jump (consumed
  // once the doc renders); ⌘↵ opens a background tab (no jump — it isn't the
  // focused reader).
  const onOpenHit = useCallback((target: HitTarget, opts: { background: boolean }) => {
    if (!opts.background) clearLibraryDocument();
    const root = useSourcesStore.getState().root;
    const ref = { path: target.path, name: target.name };
    useTabsStore.getState().openTab(ref, { activate: !opts.background, originRoot: root });
    useSourcesStore.getState().addRecent(ref, root);
    if (!opts.background && target.line != null) {
      // A target that carries no end line names one line, not an unknown extent:
      // the palette's file crumb deliberately opens at the top with no line at
      // all, and only a chunk target supplies a range.
      setJump({
        path: target.path,
        startLine: target.line,
        endLine: target.endLine ?? target.line,
        nonce: (jumpSeq.current += 1),
      });
    }
  }, [clearLibraryDocument]);

  const onNavigate = useCallback((ref: DocRef) => {
    clearLibraryDocument();
    const root = useSourcesStore.getState().root;
    const active = useTabsStore.getState().activeTabId;
    if (!active) {
      useTabsStore.getState().openTab(ref, { originRoot: root });
    } else {
      useTabsStore.getState().navigateInTab(active, ref);
    }
    useSourcesStore.getState().addRecent(ref, root);
  }, [clearLibraryDocument]);

  const onPersistScroll = useCallback((scroll: ScrollPos) => {
    const active = useTabsStore.getState().activeTabId;
    if (active) useTabsStore.getState().setScroll(active, scroll);
  }, []);

  const onDocMeta = useCallback((meta: { words: number } | null) => {
    setDocWords(meta?.words ?? null);
  }, []);

  const onMissingWikiLink = useCallback((target: string) => {
    toast(`No note named "${target}"`);
  }, []);

  const onAmbiguousWikiLink = useCallback((target: string, candidates: DocRef[]) => {
    toast(`${candidates.length} notes match "${target}" — opening the first`);
    if (candidates[0]) {
      const root = useSourcesStore.getState().root;
      useTabsStore.getState().openTab(candidates[0], { originRoot: root });
      useSourcesStore.getState().addRecent(candidates[0], root);
    }
  }, []);

  // --- semantic surfaces: transport, selection, click-to-source ---------
  const semantics = useMemo(makeSemanticsAdapter, []);
  // The registered schemas drive the toolbar's Semantic view control and the lens
  // band. Only read inside a Tauri shell — there is no engine on the website.
  const { schemas, reload: reloadSchemas } = useSchemas(
    semantics,
    root,
    isTauri(),
    importState?.phase === 'done',
  );
  const activeLens = useMemo(
    () => schemas.find((schema) => schema.name === lensSchema) ?? null,
    [schemas, lensSchema],
  );

  // Open a document at an exact line range and mark it — the one landing used by
  // every provenance affordance (a claim's citation, a projected object's
  // citation, a located violation, an explained hit). The range is carried
  // through unmodified, BOTH ends of it: a violation that knows its lines is the
  // entire point of the Constraint Explorer, so this either lands on the whole
  // range or reports that it could not, and never approximates.
  const jumpToSpan = useCallback(
    (absPath: string, name: string, startLine: number, endLine: number) => {
      clearLibraryDocument();
      const workspaceRoot = useSourcesStore.getState().root;
      const ref = { path: absPath, name };
      useTabsStore.getState().openTab(ref, { originRoot: workspaceRoot });
      useSourcesStore.getState().addRecent(ref, workspaceRoot);
      setJump({ path: absPath, startLine, endLine, nonce: (jumpSeq.current += 1) });
    },
    [clearLibraryDocument],
  );

  // Resolve a repo-relative file (how the engine cites provenance) to a path the
  // reader can open, then hand over the cited range whole.
  const openCitation = useCallback(
    (file: string, startLine: number, endLine: number) => {
      const sources = useSourcesStore.getState();
      const absPath = resolveSourcePath(file, sources.root, sources.files);
      if (absPath === null) {
        toast(`No document in this workspace matches ${file}`);
        return;
      }
      const name = absPath.split(/[\\/]+/).pop() ?? file;
      jumpToSpan(absPath, name, startLine, endLine);
    },
    [jumpToSpan],
  );

  // The Inspector's and the Semantic view sheet's citation jump. Both hand over the
  // claim's recorded first AND last line; neither is narrowed here.
  const onCite = useCallback(
    (file: string, startLine: number, endLine: number) => openCitation(file, startLine, endLine),
    [openCitation],
  );

  // The Constraint Explorer's jump. The whole located span arrives; a violation
  // with no line is not click-to-source and says so rather than opening the
  // document at the top and pretending that was the answer.
  const onJumpToViolation = useCallback(
    (span: SourceSpan) => {
      if (span.startLine === null) {
        toast(`${span.file} has no line range recorded for this violation`);
        return;
      }
      // A span with a start and NO end is a single-line location — that is what
      // `citeSourceSpan` already renders it as (`file:41`, not `file:41–?`), so
      // the marked extent matches the citation the reader clicked. It is not a
      // stand-in for an end the engine failed to record.
      openCitation(span.file, span.startLine, span.endLine ?? span.startLine);
      setConstraintsOpen(false);
    },
    [openCitation],
  );

  // The Query Explanation's open. A hit is a chunk, and a chunk is a range: the
  // reader marks the chunk that actually matched, not its opening line.
  const onOpenExplained = useCallback(
    (hit: ExplainedHit) => {
      jumpToSpan(hit.absPath, hit.name, hit.startLine, hit.endLine);
      setExplainOpen(false);
    },
    [jumpToSpan],
  );

  // Reading a selection in the reading column as a line range. Only a real,
  // non-collapsed selection inside `.reader-host` counts — a bare click is not
  // an inspection request, so the panel keeps the passage it was already on.
  useEffect(() => {
    if (!inspectorOpen) return;
    const capture = () => {
      const host = document.querySelector('.reader-host');
      const span = spanFromSelection(window.getSelection(), host);
      const path = activePathRef.current;
      if (!span || !path) return;
      setInspected((current) =>
        current &&
        current.path === path &&
        current.startLine === span.startLine &&
        current.endLine === span.endLine
          ? current
          : { path, startLine: span.startLine, endLine: span.endLine },
      );
    };
    document.addEventListener('mouseup', capture);
    document.addEventListener('keyup', capture);
    return () => {
      document.removeEventListener('mouseup', capture);
      document.removeEventListener('keyup', capture);
    };
  }, [inspectorOpen]);

  // A passage belongs to the document it was selected in; switching documents
  // clears it rather than showing another file's claims under this file's lines.
  const activeDocPath = activeTab?.docRef.path ?? null;
  useEffect(() => {
    setInspected((current) => (current && current.path === activeDocPath ? current : null));
  }, [activeDocPath]);

  const focusOutline = useCallback(() => {
    useSettingsStore.setState({ outlineVisible: true });
    requestAnimationFrame(() => {
      document.querySelector<HTMLElement>('.outline-rail a')?.focus();
    });
  }, []);

  // --- light editing (source/split, hash-guarded save, conflicts) --------
  // Cmd+E: toggle read ↔ source for the active tab's current doc.
  const toggleSourceMode = useCallback(() => {
    const tab = selectActiveTab(useTabsStore.getState());
    if (!tab) return;
    const current = useEditorStore.getState().mode(tab.id, tab.docRef.path);
    void setEditorMode(current === 'read' ? 'source' : 'read');
  }, [setEditorMode]);

  // Save the (tab, doc) entry under the hash guard (or forced, from the conflict
  // banner's explicit overwrite). A `saved` outcome re-anchors the buffer to the
  // fresh hash — which the watcher then recognises as our own write — and queues a
  // reindex; a `conflict` raises the banner rather than clobbering the on-disk
  // change. The write only ever reaches the entry's OWN document (composite key),
  // so it can never write one doc's buffer into another.
  const saveDocEntry = useCallback(
    async (tabId: string, path: string, force: boolean): Promise<'saved' | 'conflict' | 'error' | 'noop'> => {
      const entry = useEditorStore.getState().entry(tabId, path);
      if (!entry) return 'noop';
      if (!force && !entry.dirty) return 'noop'; // a clean guarded save is a no-op
      // Snapshot exactly what we send, so completion can mark THIS content saved
      // (not a newer buffer produced by typing during the in-flight save).
      const snapshot = entry.buffer;
      const result = await runSave({
        path: entry.path,
        buffer: snapshot,
        baseHash: entry.baseHash,
        force,
        save: saveDoc,
      });
      const editor = useEditorStore.getState();
      if (result.kind === 'saved') {
        editor.markSaved(tabId, path, snapshot, result.newHash);
        void triggerReindex();
        return 'saved';
      }
      if (result.kind === 'conflict') {
        editor.raiseConflict(tabId, path, result.currentHash, 'save');
        return 'conflict';
      }
      toast(`Could not save — ${result.message}`);
      return 'error';
    },
    [triggerReindex],
  );

  // Save the active tab's CURRENT document (Cmd+S / the editor's Cmd+S). If the
  // tab has navigated to a doc with no entry, this is a no-op — a buffer for a doc
  // the tab left is never written to the doc it now shows.
  const saveActiveDoc = useCallback(
    (force: boolean) => {
      const tab = selectActiveTab(useTabsStore.getState());
      if (!tab) return Promise.resolve('noop' as const);
      return saveDocEntry(tab.id, tab.docRef.path, force);
    },
    [saveDocEntry],
  );

  // Conflict banner "Reload": discard the local buffer and adopt the on-disk
  // version (re-seeding the anchor), and refresh the reader view too.
  const reloadActiveDoc = useCallback(async () => {
    const tab = selectActiveTab(useTabsStore.getState());
    if (!tab) return;
    const tabId = tab.id;
    const docPath = tab.docRef.path;
    try {
      const doc = await readDoc(docPath);
      const editor = useEditorStore.getState();
      const mode = editor.mode(tabId, docPath);
      editor.seed(tabId, docPath, doc.content, doc.hash, mode === 'read' ? 'source' : mode);
      setReloadNonce((nonce) => nonce + 1);
    } catch (cause) {
      toast(`Could not reload — ${cause instanceof Error ? cause.message : String(cause)}`);
    }
  }, []);

  // Close a tab, dropping ALL of its editor buffers. The single place a tab is
  // torn down (reached only for a clean tab or after the guard resolves).
  const closeTabAndBuffer = useCallback((tabId: string) => {
    clearLibraryDocument();
    useEditorStore.getState().removeTab(tabId);
    useTabsStore.getState().closeTab(tabId);
  }, [clearLibraryDocument]);

  // Request a tab close: a tab with ANY unsaved document prompts the guard first;
  // a clean one closes immediately. Every close path (⌘W, the tab's ×, middle
  // click) routes through here, so a dirty buffer is never silently discarded.
  const requestCloseTab = useCallback(
    (tabId: string) => {
      clearLibraryDocument();
      if (useEditorStore.getState().tabIsDirty(tabId)) setCloseGuard({ kind: 'tab', tabId });
      else closeTabAndBuffer(tabId);
    },
    [clearLibraryDocument, closeTabAndBuffer],
  );

  // Save every dirty document of a tab (Save on the tab-close guard).
  const saveTabAll = useCallback(
    async (tabId: string): Promise<boolean> => {
      let allClean = true;
      for (const entry of useEditorStore.getState().allDirtyEntries().filter((e) => e.tabId === tabId)) {
        const kind = await saveDocEntry(entry.tabId, entry.path, false);
        if (kind !== 'saved' && kind !== 'noop') allClean = false;
      }
      return allClean;
    },
    [saveDocEntry],
  );

  // Save every dirty document across all tabs (Save on the quit / switch guard).
  const saveAllDirty = useCallback(async (): Promise<boolean> => {
    let allClean = true;
    for (const entry of useEditorStore.getState().allDirtyEntries()) {
      const kind = await saveDocEntry(entry.tabId, entry.path, false);
      if (kind !== 'saved' && kind !== 'noop') allClean = false;
    }
    return allClean;
  }, [saveDocEntry]);

  // Complete a quit the guard approved (Save-then-close or Discard). The session
  // was already drained in the close handler before the prompt; here we clear any
  // remaining dirty state and force the window down. We use `destroy()` (not
  // `close()`): the window was already `preventDefault`'d in onCloseRequested, and
  // `close()` merely re-requests a close the webview won't honour; `destroy()`
  // tears it down unconditionally.
  const performQuit = useCallback(async () => {
    useEditorStore.getState().reset(); // discard-all: no dirty documents remain
    closingRef.current = true;
    try {
      await getCurrentWindow().destroy();
    } catch {
      /* not a real Tauri window (mocked IPC) — nothing to close */
    }
  }, []);

  // The destructive action a resolved guard performs. Reached ONLY when the user
  // chose to proceed (Discard, or Save that landed genuinely clean without a
  // cancel racing in).
  const proceedAfterGuard = useCallback(
    (
      guard:
        | { kind: 'tab'; tabId: string }
        | { kind: 'quit' }
        | {
            kind: 'switch';
            folder: string;
            sourceId?: string;
            expectedSourceIntent: number;
            resume?: {
              kind: 'library-edit';
              member: LibrarySourceExecution;
              row: EntityRow;
            };
          },
    ) => {
      if (guard.kind === 'tab') closeTabAndBuffer(guard.tabId);
      else if (guard.kind === 'quit') void performQuit();
      else {
        void (async () => {
          const committed = await commitSwitchRef.current(
            guard.folder,
            guard.sourceId,
            guard.expectedSourceIntent,
          );
          if (
            committed &&
            guard.resume?.kind === 'library-edit' &&
            sourceIntentRef.current === guard.expectedSourceIntent
          ) {
            await editLibraryDocumentRef.current(
              guard.resume.member,
              guard.resume.row,
              guard.expectedSourceIntent,
            );
          }
        })();
      }
    },
    [closeTabAndBuffer, performQuit],
  );

  // A live mirror of the guard so an in-flight Save can detect a Cancel (or a
  // replacement) that landed during its await.
  const closeGuardRef = useRef(closeGuard);
  closeGuardRef.current = closeGuard;

  // Guard "Save" — TRANSACTIONAL: save, then re-check. If the user cancelled
  // during the save (the guard changed), or a save conflicted, or typing during
  // the in-flight save left the work dirty, do NOT proceed. Only a genuinely-clean
  // result closes/quits/switches.
  const onGuardSave = useCallback(async () => {
    const guard = closeGuardRef.current;
    if (!guard) return;
    const allClean = guard.kind === 'tab' ? await saveTabAll(guard.tabId) : await saveAllDirty();
    // Cancel (or a new guard) raced in during the save → abort the whole thing.
    if (closeGuardRef.current !== guard) return;
    const stillDirty =
      guard.kind === 'tab'
        ? useEditorStore.getState().tabIsDirty(guard.tabId)
        : useEditorStore.getState().dirtyTabIds().length > 0;
    setCloseGuard(null);
    if (allClean && !stillDirty) proceedAfterGuard(guard);
  }, [saveTabAll, saveAllDirty, proceedAfterGuard]);

  // Guard "Discard": proceed, losing the unsaved edits.
  const onGuardDiscard = useCallback(() => {
    const guard = closeGuardRef.current;
    if (!guard) return;
    setCloseGuard(null);
    proceedAfterGuard(guard);
  }, [proceedAfterGuard]);

  // Bound editor memory. Keep an entry only when it is DIRTY (never drop an
  // unsaved edit) OR it is a live tab's currently-active document. This reclaims
  // entries for closed/retired tabs (already guard-handled if they were dirty) AND
  // evicts clean buffers for docs a tab has navigated away from — so a long-lived
  // tab that has viewed many docs in source mode does not grow unbounded.
  useEffect(() => {
    const activePathByTab = new Map(tabs.map((tab) => [tab.id, tab.docRef.path]));
    useEditorStore
      .getState()
      .retain((tabId, path, dirty) => dirty || activePathByTab.get(tabId) === path);
  }, [tabs]);

  // --- the one keymap registry (dispatch + sheet) -----------------------
  const registry = useMemo(() => {
    const map = new KeymapRegistry(isMac);
    const tabsApi = () => useTabsStore.getState();
    const bindings: Binding[] = [
      {
        id: 'jump',
        keys: 'Mod+P',
        description: 'Jump to file',
        group: 'Files',
        run: () => {
          setPaletteMode('file');
          setPaletteOpen(true);
        },
      },
      {
        id: 'search',
        keys: 'Mod+K',
        description: 'Search',
        group: 'Search',
        run: () => {
          setPaletteMode('search');
          setPaletteOpen(true);
        },
      },
      {
        id: 'related',
        keys: 'Mod+Shift+R',
        description: 'Toggle related rail',
        group: 'Search',
        run: () => useSettingsStore.getState().toggleRelatedRail(),
      },
      {
        id: 'explain',
        keys: 'Mod+Shift+K',
        description: 'Explain a search',
        group: 'Search',
        run: () => setExplainOpen((open) => !open),
      },
      {
        id: 'view-as',
        keys: 'Mod+Shift+V',
        description: 'Semantic view…',
        group: 'Semantics',
        run: () => setLensSheetOpen((open) => !open),
      },
      {
        id: 'inspector',
        keys: 'Mod+Shift+I',
        description: 'Toggle semantic inspector',
        group: 'Semantics',
        run: () => useSettingsStore.getState().toggleInspector(),
      },
      {
        id: 'laws',
        keys: 'Mod+Shift+L',
        description: 'Where the laws break',
        group: 'Semantics',
        run: () => setConstraintsOpen((open) => !open),
      },
      {
        id: 'new-tab',
        keys: 'Mod+T',
        description: 'New tab',
        group: 'Tabs',
        run: () => {
          setPaletteMode('file');
          setPaletteOpen(true);
        },
      },
      {
        id: 'close-tab',
        keys: 'Mod+W',
        description: 'Close tab',
        group: 'Tabs',
        run: () => {
          const id = tabsApi().activeTabId;
          if (id) requestCloseTab(id); // dirty → unsaved-changes guard
        },
      },
      {
        id: 'reopen-tab',
        keys: 'Mod+Shift+T',
        description: 'Reopen closed tab',
        group: 'Tabs',
        run: () => tabsApi().reopenClosedTab(),
      },
      {
        id: 'edit-source',
        keys: 'Mod+E',
        description: 'Toggle source view',
        group: 'Editing',
        run: toggleSourceMode,
      },
      {
        id: 'save',
        keys: 'Mod+S',
        description: 'Save document',
        group: 'Editing',
        run: () => void saveActiveDoc(false),
      },
      {
        id: 'mru-next',
        keys: 'Ctrl+Tab',
        description: 'Next tab (recent)',
        group: 'Tabs',
        run: () => tabsApi().cycleMru(1),
      },
      {
        id: 'mru-prev',
        keys: 'Ctrl+Shift+Tab',
        description: 'Previous tab (recent)',
        group: 'Tabs',
        run: () => tabsApi().cycleMru(-1),
      },
      {
        id: 'back',
        keys: 'Mod+[',
        description: 'Back',
        group: 'Navigation',
        run: () => {
          const id = tabsApi().activeTabId;
          if (id) tabsApi().back(id);
        },
      },
      {
        id: 'forward',
        keys: 'Mod+]',
        description: 'Forward',
        group: 'Navigation',
        run: () => {
          const id = tabsApi().activeTabId;
          if (id) tabsApi().forward(id);
        },
      },
      {
        id: 'outline',
        keys: 'Mod+Shift+O',
        description: 'Focus outline',
        group: 'Navigation',
        run: focusOutline,
      },
      {
        id: 'sidebar',
        keys: 'Mod+\\',
        description: 'Toggle sidebar',
        group: 'View',
        run: () => useSettingsStore.getState().toggleSidebar(),
      },
      {
        id: 'focus',
        keys: 'Mod+.',
        description: 'Focus mode',
        group: 'View',
        run: () => useSettingsStore.getState().toggleFocusMode(),
      },
      {
        id: 'settings',
        keys: 'Mod+,',
        description: 'Settings',
        group: 'View',
        allowInInput: true,
        run: () => setSettingsOpen(true),
      },
      {
        id: 'shortcuts',
        keys: 'Mod+/',
        description: 'Keyboard shortcuts',
        group: 'View',
        run: () => setSheetOpen((open) => !open),
      },
    ];
    map.registerAll(bindings);
    return map;
  }, [isMac, focusOutline, toggleSourceMode, saveActiveDoc, requestCloseTab]);

  const shortcutGroups = useMemo<ShortcutGroup[]>(
    () =>
      registry.groups().map((group) => ({
        group: group.group,
        items: group.bindings.map((binding) => ({
          id: binding.id,
          description: binding.description,
          keys: registry.display(binding),
        })),
      })),
    [registry],
  );

  // --- global keyboard dispatch ----------------------------------------
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        if (paletteOpen) {
          setPaletteOpen(false);
          event.preventDefault();
          return;
        }
        if (sheetOpen) {
          setSheetOpen(false);
          event.preventDefault();
          return;
        }
        if (settingsOpen) {
          setSettingsOpen(false);
          event.preventDefault();
          return;
        }
        // The semantic sheets dismiss in the same one-at-a-time order as the
        // rest of the overlay stack, ahead of leaving focus mode.
        if (lensSheetOpen || constraintsOpen || explainOpen) {
          setLensSheetOpen(false);
          setConstraintsOpen(false);
          setExplainOpen(false);
          event.preventDefault();
          return;
        }
        if (useSettingsStore.getState().focusMode) {
          useSettingsStore.getState().setFocusMode(false);
          event.preventDefault();
          return;
        }
      }
      const inEditable =
        isEditable(document.activeElement) ||
        paletteOpen ||
        sheetOpen ||
        settingsOpen ||
        lensSheetOpen ||
        constraintsOpen ||
        explainOpen;
      if (registry.dispatch(event, { inEditable })) event.preventDefault();
    }
    function onKeyUp(event: KeyboardEvent) {
      if (event.key === 'Control') useTabsStore.getState().endMruCycle();
    }
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('keyup', onKeyUp);
    return () => {
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('keyup', onKeyUp);
    };
  }, [
    registry,
    paletteOpen,
    sheetOpen,
    settingsOpen,
    lensSheetOpen,
    constraintsOpen,
    explainOpen,
  ]);

  // --- startup: restore session + settings ------------------------------
  useEffect(() => {
    let active = true;
    const startupIntent = sourceIntentRef.current;
    const startupIsCurrent = () =>
      active && sourceIntentRef.current === startupIntent;
    void (async () => {
      let persisted: Awaited<ReturnType<typeof loadPersisted>>;
      try {
        persisted = await loadPersisted();
      } catch {
        if (startupIsCurrent()) {
          toast('The saved session could not be read; startup restore was skipped');
        }
        if (active) setHydrated(true);
        return;
      }
      const { session, settings } = persisted;
      if (Object.keys(settings).length > 0) useSettingsStore.getState().hydrate(settings);
      if (!startupIsCurrent()) {
        if (active) setHydrated(true);
        return;
      }
      if (session) {
        useTabsStore.getState().hydrate(session.tabs, session.activeTabId);
        // A serialized path is history, not a filesystem grant. Keep its Recent
        // metadata, but leave the live root empty until backend-owned membership
        // confirms this exact source still exists and is available.
        useSourcesStore.getState().hydrate({ root: null, recent: session.recent });
        if (session.root && isTauri()) {
          try {
            const member = await reconcileLibraryRestore(session.root);
            if (startupIsCurrent() && member) {
              // The registry returns its current canonical path. Route that member
              // through the SAME watch/epoch transaction as an interactive switch,
              // then mark it active only after the grant commits.
              await commitSwitchRef.current(member.path, member.id, startupIntent);
            }
          } catch {
            if (startupIsCurrent()) {
              toast('The saved library could not be reconciled; no folder was opened');
            }
          }
        }
      }
      if (isTauri()) recordDiagnosticEventNonBlocking({ kind: 'application-started' });
      setHydrated(true);
    })();
    return () => {
      active = false;
    };
  }, []);

  // --- theme application ------------------------------------------------
  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  // --- persist session --------------------------------------------------
  // Enqueue the current session onto the single-flight writer, reading the latest
  // store state at call time. The writer serializes every persist (debounced, on
  // visibility change, on close) so writes never overlap and a burst coalesces to
  // the newest snapshot — no interleaving, no lost final state.
  const flushSession = useCallback(() => {
    const t = useTabsStore.getState();
    const s = useSourcesStore.getState();
    sessionWriter.write(
      serializeSession({
        root: s.root,
        activeTabId: t.activeTabId,
        tabs: t.tabs,
        recent: s.recent,
      }),
    );
  }, []);
  flushSessionRef.current = flushSession;

  // Persist the current settings NOW (bypassing the 1s debounce), reading the
  // latest store state at call time. Returns the write promise so the close path
  // can bound-await it. Settings are small and last-writer-wins, so an immediate
  // write-through is safe; without this, a preference toggled in the last second
  // before the app closes (theme, reading font, text size, the related rail) is
  // lost when only the session — not settings — is flushed on the way out.
  const flushSettings = useCallback(() => {
    const s = useSettingsStore.getState();
    return saveSettings({
      theme: s.theme,
      readingFont: s.readingFont,
      textScaleIndex: s.textScaleIndex,
      sidebarCollapsed: s.sidebarCollapsed,
      outlineVisible: s.outlineVisible,
      relatedRailOpen: s.relatedRailOpen,
      inspectorOpen: s.inspectorOpen,
      lensSchema: s.lensSchema,
      reviewer: s.reviewer,
    });
  }, []);

  // Debounced 1s on every session-shaping change.
  useEffect(() => {
    if (!hydrated) return;
    const handle = window.setTimeout(flushSession, 1000);
    return () => window.clearTimeout(handle);
  }, [hydrated, flushSession, tabs, activeTabId, root, recent]);

  // Flush synchronously when the window is going away, so quitting within the 1s
  // debounce window can't restore a stale (pre-switch) workspace/tab set. Both
  // signals are best-effort: `visibilitychange:hidden` is the reliable
  // "app backgrounding/closing" cue, `beforeunload` covers a plain webview close.
  useEffect(() => {
    if (!hydrated) return;
    const flushAll = () => {
      flushSession();
      void flushSettings();
    };
    const onBeforeUnload = () => flushAll();
    const onVisibility = () => {
      if (document.visibilityState === 'hidden') flushAll();
    };
    window.addEventListener('beforeunload', onBeforeUnload);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.removeEventListener('beforeunload', onBeforeUnload);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [hydrated, flushSession, flushSettings]);

  // Native window close (the red X). We intercept to (a) honour the persistence
  // contract — a BOUNDED session drain before the window goes away — and (b) guard
  // unsaved edits. Because we preventDefault to do that async work, we must then
  // force the close ourselves, and we do so with destroy() (which requires the
  // `core:window:allow-destroy` capability), NOT close(): a window we just vetoed
  // will not honour a fresh close() in the real webview. The `closing` flag guards
  // against a second close request racing in before destroy() completes:
  //   - re-entry (`closing` already set) → do NOT preventDefault → let it through.
  //   - first entry → preventDefault; flush + BOUNDED drain (never hang the close);
  //     if dirty → prompt (the guard clears dirty then force-closes via destroy());
  //     else mark `closing` and call destroy() to tear the window down.
  // Net: the red X ALWAYS closes, after a bounded drain, via destroy().
  useEffect(() => {
    if (!hydrated || !isTauri()) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void (async () => {
      try {
        const appWindow = getCurrentWindow();
        const off = await appWindow.onCloseRequested(async (event) => {
          if (closingRef.current) return; // re-entry: let the native close proceed
          event.preventDefault();
          // Persistence contract: flush the latest session AND settings, then DRAIN
          // them, bounded so a stuck write can never wedge the close. Settings ride
          // the same barrier as the session so a last-second preference change
          // (theme, font, text size, related rail) survives an immediate quit.
          flushSession();
          const settingsWrite = flushSettings();
          await Promise.race([
            Promise.all([sessionWriter.drain(), settingsWrite]),
            new Promise((resolve) => setTimeout(resolve, 2000)),
          ]);
          if (useEditorStore.getState().dirtyTabIds().length > 0) {
            setCloseGuard({ kind: 'quit' }); // guard clears dirty + re-closes
            return;
          }
          closingRef.current = true;
          // Force the close PAST our own preventDefault. `close()` only re-requests
          // a close (re-firing onCloseRequested) and, in the real webview, does not
          // actually shut a window we just vetoed — the mock that "re-fires" misled
          // the unit tests; the macOS GUI smoke is the real guard. `destroy()` tears
          // the window down unconditionally without re-firing (needs allow-destroy).
          await appWindow.destroy();
        });
        if (disposed) off();
        else unlisten = off;
      } catch {
        /* not a real Tauri window (mocked IPC / no window API) — no close hook */
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [hydrated, flushSession, flushSettings]);

  // --- persist settings (debounced 1s) ----------------------------------
  useEffect(() => {
    if (!hydrated) return;
    const handle = window.setTimeout(() => void flushSettings(), 1000);
    return () => window.clearTimeout(handle);
  }, [
    hydrated,
    flushSettings,
    theme,
    readingFont,
    textScaleIndex,
    sidebarCollapsed,
    outlineVisible,
    relatedRailOpen,
    inspectorOpen,
    lensSchema,
    reviewer,
  ]);

  // --- live reload from the workspace watcher ---------------------------
  // A "continuous" write burst (a long save) can fire many events for one path.
  // We debounce per-path on the trailing edge (~300ms quiet window) so a single
  // logical edit yields one reload + one toast, never a flurry of partial
  // renders. A settled ("any") change flushes immediately.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let refreshTimer: number | undefined;
    const reloadTimers = new Map<string, number>();

    // The live open + watcher generations, for re-validating a timer at fire.
    const nowStamp = () => ({ openGen: openEpochRef.current, watcherGen: watcherGenRef.current });

    const reloadActive = async (path: string, stamp: { openGen: number; watcherGen: number }) => {
      // A switch between event arrival and this (debounced) fire discards it —
      // BOTH the open generation and the watcher generation must still match.
      if (!watcherStampCurrent(stamp, nowStamp())) return;
      if (path !== activePathRef.current) return;

      const tabId = activeTabIdRef.current;
      const entry = tabId ? useEditorStore.getState().entry(tabId, path) : undefined;
      // Read mode / not editing this doc: the existing live-refresh behavior.
      if (!tabId || !entry) {
        setReloadNonce((nonce) => nonce + 1);
        toast('Updated');
        return;
      }

      // Editing this doc: classify the disk change against our anchor. Reading the
      // hash tells our OWN just-saved write (baseHash) apart from a real external
      // change — the self-write suppression the plan calls for.
      let disk: { content: string; hash: string };
      try {
        disk = await readDoc(path);
      } catch {
        return;
      }
      // Re-validate after the await: a switch (or a doc change) may have landed.
      if (!watcherStampCurrent(stamp, nowStamp()) || path !== activePathRef.current) return;
      const editor = useEditorStore.getState();
      const live = editor.entry(tabId, path);
      if (!live) {
        setReloadNonce((nonce) => nonce + 1);
        toast('Updated');
        return;
      }
      const decision = classifyExternalChange(disk.hash, live.baseHash, live.dirty);
      if (decision === 'self') return; // our own write echoing back — stay silent
      if (decision === 'conflict') {
        // Unsaved edits + a divergent disk: never clobber — raise the banner.
        editor.raiseConflict(tabId, path, disk.hash, 'external');
        return;
      }
      // Clean buffer: safe to adopt disk. Re-seed the anchor and refresh the view.
      editor.seed(tabId, path, disk.content, disk.hash, live.mode);
      setReloadNonce((nonce) => nonce + 1);
      toast('Updated');
    };

    onFsChanged((change) => {
      // Discard events from a PREVIOUS watcher (still draining after a switch) —
      // they belong to a workspace we have moved off.
      if (change.generation !== watcherGenRef.current) return;
      // Stamp the timer at event ARRIVAL with BOTH generations; the debounce
      // timers below re-validate BOTH at fire, so a switch mid-debounce (either
      // counter advancing) discards them rather than re-tagging old work.
      const stamp = { openGen: openEpochRef.current, watcherGen: change.generation };
      const existing = reloadTimers.get(change.path);
      if (existing !== undefined) window.clearTimeout(existing);
      if (change.kind === 'continuous') {
        // Still being written — wait for the quiet window before reloading.
        reloadTimers.set(
          change.path,
          window.setTimeout(() => {
            reloadTimers.delete(change.path);
            void reloadActive(change.path, stamp);
          }, 300),
        );
      } else {
        // Settled: drop any pending trailing reload and apply now.
        reloadTimers.delete(change.path);
        void reloadActive(change.path, stamp);
      }

      window.clearTimeout(refreshTimer);
      refreshTimer = window.setTimeout(() => {
        // Re-validate BOTH generations at fire: a switch during the debounce
        // window means this event belongs to a workspace we have left → discard.
        if (!watcherStampCurrent(stamp, nowStamp())) return;
        // A settled change drives an incremental import THEN a re-list (queued as
        // a follow-up when an import is already running) so the index — and thus
        // wikilink/title resolution — stays in step with disk.
        void triggerReindex();
      }, 400);
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {
        /* not in a Tauri webview — no watcher */
      });
    return () => {
      unlisten?.();
      window.clearTimeout(refreshTimer);
      for (const timer of reloadTimers.values()) window.clearTimeout(timer);
    };
  }, [triggerReindex]);

  // --- watcher runtime errors → degraded status -------------------------
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    onWatchError((error) => {
      // An OLD watcher's error must not mark the CURRENT watcher failed.
      if (error.generation !== watcherGenRef.current) return;
      useSourcesStore.getState().setWatcher('error');
      const sourceId = activeSourceIdRef.current;
      if (sourceId) {
        recordDiagnosticEventNonBlocking({
          kind: 'watcher',
          source_id: sourceId,
          state: 'error',
          error: 'watcher-unavailable',
        });
      }
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {
        /* not in a Tauri webview — no watcher */
      });
    return () => {
      unlisten?.();
    };
  }, []);

  // Recent is scoped to the active workspace: an entry retired from workspace A
  // never surfaces (and so can never be reopened as an error tab) under B's grant.
  const visibleRecent = useMemo(
    () =>
      recent
        .filter((entry) => entry.root === root)
        .map((entry) => ({ path: entry.path, name: entry.name })),
    [recent, root],
  );

  // Gitignored files carry an `ignored` flag from the fs-fallback listing; the
  // tree dims them. (Index-listed files are never ignored — the scan drops them.)
  const ignoredPaths = useMemo(() => {
    const set = new Set<string>();
    for (const file of files) if (file.ignored) set.add(file.path);
    return set;
  }, [files]);
  const isIgnored = useCallback((path: string) => ignoredPaths.has(path), [ignoredPaths]);

  const textScale = textScaleFactor({ textScaleIndex });

  // Palette + rail context. `sourceName` (the workspace's basename) is the
  // breadcrumb's `source` segment; `paletteFiles` feeds file mode; `activeDoc`
  // (the focused tab) drives the Related intent and the related rail.
  const sourceName = useMemo(() => {
    if (!root) return null;
    const parts = root.replace(/[\\/]+$/, '').split(/[\\/]+/);
    return parts[parts.length - 1] ?? root;
  }, [root]);
  const paletteFiles = useMemo<PaletteFile[]>(
    () => files.map((file) => ({ path: file.path, name: file.name })),
    [files],
  );
  const activeDoc = useMemo(
    () => (activeTab ? { path: activeTab.docRef.path, name: activeTab.docRef.name } : null),
    [activeTab],
  );
  // The search/related transport, closed over the live watcher generation so
  // streamed batches from a superseded workspace epoch are discarded.
  const adapter = useMemo(() => makeSearchAdapter(() => watcherGenRef.current), []);

  return (
    <div className={`shell${focusMode ? ' shell--focus' : ''}`}>
      <Titlebar
        isMac={isMac}
        sidebarCollapsed={sidebarCollapsed}
        onToggleSidebar={() => useSettingsStore.getState().toggleSidebar()}
        theme={theme}
        onCycleTheme={() => useSettingsStore.getState().cycleTheme()}
        readingFont={readingFont}
        onToggleFont={() => useSettingsStore.getState().toggleReadingFont()}
        onDecreaseText={() => useSettingsStore.getState().decreaseTextSize()}
        onIncreaseText={() => useSettingsStore.getState().increaseTextSize()}
        onToggleFocus={() => useSettingsStore.getState().toggleFocusMode()}
        onShowSettings={() => setSettingsOpen(true)}
        onShowShortcuts={() => setSheetOpen(true)}
      />

      <div className="shell__body">
        <Sidebar
          collapsed={sidebarCollapsed}
          root={root}
          files={files}
          recent={visibleRecent}
          activePath={activeTab?.docRef.path ?? null}
          loading={loading}
          listingError={listingError}
          onPickFolder={onPickFolder}
          onOpenFile={openFile}
          onOpenRecent={openRef}
          isIgnored={isIgnored}
          libraryEvidence={libraryEvidence}
          librarySearch={librarySearch}
          libraryLoading={libraryLoading}
          libraryError={libraryError}
          onLibrarySearch={setLibrarySearch}
          onOpenLibrary={(member, row) => {
            setLibraryError(null);
            const evidence = libraryEvidence.find((answer) =>
              answer.memberships.some(
                (candidate) => candidate.membership_id === member.membership_id,
              ),
            );
            if (evidence) {
              setLibraryDocument({ member, row, evidence });
            }
          }}
          onRefreshLibrary={() => void refreshLibrary()}
        />

        <main className="shell__main">
          <TabStrip
            onLocalIntent={clearLibraryDocument}
            onRequestClose={requestCloseTab}
          />
          <div
            className={`shell__reader-row${
              relatedRailOpen ? ' shell__reader-row--related-open' : ''
            }${libraryDocument ? ' shell__reader-row--library-open' : ''
            }`}
          >
            {libraryDocument ? (
              <LibraryDocumentPane
                member={libraryDocument.member}
                row={libraryDocument.row}
                evidence={libraryDocument.evidence}
                onRetry={() => void refreshLibrary()}
                onEdit={
                  libraryDocument.member.editable
                    ? () =>
                        void editLibraryDocument(
                          libraryDocument.member,
                          libraryDocument.row,
                        )
                    : undefined
                }
              />
            ) : (
            <>
              <ReaderPane
              tab={activeTab ?? null}
              root={root}
              files={files}
              outlineVisible={outlineVisible}
              readingFont={readingFont}
              textScale={textScale}
              reloadNonce={reloadNonce}
              jump={jump}
              onToggleOutline={() => useSettingsStore.getState().toggleOutline()}
              onNavigate={onNavigate}
              onPersistScroll={onPersistScroll}
              onDocMeta={onDocMeta}
              onPickFolder={onPickFolder}
              onMissingWikiLink={onMissingWikiLink}
              onAmbiguousWikiLink={onAmbiguousWikiLink}
              onSetMode={(mode) => void setEditorMode(mode)}
              onSaveActive={() => void saveActiveDoc(false)}
              onReloadActive={() => void reloadActiveDoc()}
              onOverwriteActive={() => void saveActiveDoc(true)}
              onViewAs={schemas.length > 0 ? () => setLensSheetOpen(true) : undefined}
              lensSchema={activeLens?.name ?? null}
              lensBand={
                activeLens ? (
                  <LensBand
                    schema={activeLens}
                    onOpen={() => setLensSheetOpen(true)}
                    onClear={() => useSettingsStore.getState().setLensSchema(null)}
                  />
                ) : null
              }
              />
            {/* The apparatus sits nearest the text, navigation outside it. */}
            {activeTab ? (
              <InspectorRail
                adapter={semantics}
                open={inspectorOpen}
                onToggle={() => useSettingsStore.getState().toggleInspector()}
                selection={inspected}
                reviewer={reviewer}
                onSetReviewer={(name) => useSettingsStore.getState().setReviewer(name)}
                onCite={onCite}
                workspaceKey={root}
                workspaceGeneration={semanticWorkspaceGeneration}
                documentPath={activeTab.docRef.path}
                queryDocument={semantics.queryFederatedDocument}
                cancelDocument={semantics.cancelFederatedDocument}
              />
            ) : null}
            {activeTab ? (
              <RelatedRail
                activeDoc={activeDoc}
                adapter={adapter}
                open={relatedRailOpen}
                onToggle={() => useSettingsStore.getState().toggleRelatedRail()}
                onOpen={openRef}
                workspaceKey={root}
              />
            ) : null}
            </>
            )}
          </div>
        </main>
      </div>

      <StatusBar
        words={docWords}
        watcher={watcher}
        fileCount={files.length}
        listingError={listingError}
        importState={importState}
        fromIndex={fromIndex}
        onRetryIndex={retryIndex}
      />

      {paletteOpen ? (
        <SearchPalette
          mode={paletteMode}
          files={paletteFiles}
          activeDoc={activeDoc}
          sourceName={sourceName}
          workspaceKey={root}
          adapter={adapter}
          isMac={isMac}
          onOpenHit={onOpenHit}
          onOpenFile={onOpenPaletteFile}
          onClose={() => setPaletteOpen(false)}
        />
      ) : null}
      {sheetOpen ? (
        <ShortcutSheet groups={shortcutGroups} onClose={() => setSheetOpen(false)} />
      ) : null}
      {settingsOpen ? (
        <SettingsSheet
          watcher={watcher}
          importState={importState}
          onActivateSource={(source) => switchWorkspace(source.path, source.id)}
          onSourceIntent={beginSourceIntent}
          onLibraryChanged={() => void refreshLibrary()}
          onClose={() => setSettingsOpen(false)}
        />
      ) : null}
      {lensSheetOpen ? (
        <LensSheet
          adapter={semantics}
          activeSchema={lensSchema}
          onChoose={(schema) => {
            useSettingsStore.getState().setLensSchema(schema);
            // Counts move as claims are ruled on; re-read so the band never
            // reports a total the graph no longer holds.
            reloadSchemas();
          }}
          onCite={onCite}
          onClose={() => setLensSheetOpen(false)}
        />
      ) : null}
      {constraintsOpen ? (
        <ConstraintSheet
          adapter={semantics}
          onJump={onJumpToViolation}
          onClose={() => setConstraintsOpen(false)}
        />
      ) : null}
      {explainOpen ? (
        <ExplainSheet
          adapter={semantics}
          onOpen={onOpenExplained}
          onClose={() => setExplainOpen(false)}
        />
      ) : null}
      {closeGuard ? (
        <UnsavedGuard
          docs={useEditorStore
            .getState()
            .allDirtyEntries()
            .filter((entry) => closeGuard.kind !== 'tab' || entry.tabId === closeGuard.tabId)
            .map((entry) => entry.path)}
          onSave={() => void onGuardSave()}
          onDiscard={onGuardDiscard}
          onCancel={() => setCloseGuard(null)}
        />
      ) : null}
      <Toasts />
    </div>
  );
}
