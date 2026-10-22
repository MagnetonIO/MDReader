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
import { DraftRecovery } from '../features/draft-recovery/DraftRecovery';
import { useDraftRecovery, retireEditorDrafts, flushEditorDrafts, resumeEditorDrafts } from '../features/draft-recovery/runtime';
import { textSizeChord } from './text-size-chord';
import { editorAdmission } from '../features/editor-preservation/admission';
import type { EditorMode } from '../stores/editor.js';
import { classifyExternalChange, runSave } from './edit-save.js';
import { useSourcesStore } from '../stores/sources.js';
import {
  applyTheme,
  textScaleFactor,
  useSettingsStore,
} from '../stores/settings.js';
import {
  documentKey,
  isLocalDocument,
  type DocRef,
  type DocumentRef,
  type ScrollPos,
} from '../stores/types.js';
import { getCurrentWindow } from '@tauri-apps/api/window';

import { isTauri, loadPersisted, sessionWriter } from './persistence.js';
import { useAppPersistence } from './use-app-persistence.js';
import {
  closeAndForgetLibrarySource,
  grantLibrarySource,
  markLibrarySourceActive,
  pickAndAddLibrarySource,
  reconcileLibraryRestore,
  recordDiagnosticEventNonBlocking,
  watchLibrarySource,
  type LibrarySourceDto,
} from '../platform/settings.js';
import { readerBackend } from '../platform/backend.js';
import { createReindexer } from './reindex.js';
import { useReadOnlyRoot } from './use-read-only-root.js';
import { LoadingGate } from './loading-gate.js';
import { watcherStampCurrent } from './watcher-guard.js';
import { performSwitch } from './switch-workspace.js';
import { Titlebar } from './Titlebar.js';
import { Sidebar } from './Sidebar.js';
import { useCreatedProviderEdit } from '../features/provider-editing/useCreatedProviderEdit';
import { CatalogSidebar } from '../features/catalog/CatalogSidebar.js';
import { catalogBackend } from '../features/catalog/backend.js';
import { providerTarget } from './provider-reading.js';
import { ReaderTabs } from '../features/reader-history/ReaderTabs';
import { ReaderRecents } from '../features/reader-history/ReaderRecents';
import { useCanonicalHistory } from '../features/reader-history/useCanonicalHistory';
import { useReaderHistoryStore } from '../features/reader-history/store';
import { restoreReaderHistory } from '../features/reader-history/serialization';
import { createTabCycle, readerActive, stepReaderTab } from '../features/reader-history/tab-actions';
import { ReaderPane } from './ReaderPane.js';
import type { JumpTarget } from './ReaderPane.js';
import { RelatedRail } from './RelatedRail.js';
import { StatusBar } from './StatusBar.js';
import { ShortcutSheet } from './ShortcutSheet.js';
import { SettingsSheet } from './SettingsSheet.js';
import { ProviderReaderDialog } from './ProviderReaderDialog.js';
import { ArchivedReviewsPanel } from '../reviews/ArchivedReviewsPanel';
import { useArchivedReviewSelection } from '../reviews/useArchivedReviewSelection';
import type { ProviderReaderReference } from '../platform/providers.js';
import { LegacyRecordNotice } from '../features/migration/LegacyRecordNotice';
import type { ShortcutGroup } from './ShortcutSheet.js';
import { UnsavedGuard } from './UnsavedGuard.js';
import { Toasts, toast } from './toasts.js';
import { CanonicalReader } from '../features/reader/CanonicalReader';
import { DeviceReader } from '../features/device-reader/DeviceReader';
import { isDeviceReference } from '../features/device-reader/backend';
import { useCatalogEditing, type CatalogEditResume } from '../features/catalog/useCatalogEditing';
import { useProviderEditing } from '../features/provider-editing/useProviderEditing';
import { ProviderEditor } from '../features/provider-editing/ProviderEditor';
import { DEVICE_READ_CHANGED } from '../features/device-reader/changed';
import { useCurrentProviderReopen } from '../features/provider-editing/useCurrentProviderReopen';
import { ProviderEditsTray } from '../features/provider-editing/ProviderEditsTray';
import { findProviderEdit, forgetProviderTab, providerTabSaving, retainedProviderEntry, saveProviderEdit, useProviderEdits } from '../features/provider-editing/store';
import { useDeviceEditing } from '../features/device-editing/useDeviceEditing';
import { DeviceEditor } from '../features/device-editing/DeviceEditor';
import { keepDraftForReading } from '../features/editor-controls/useEditorExit';
import { AgentSessionsPane } from '../features/agent-sessions/AgentSessionsPane';
import { DeviceEditsTray } from '../features/device-editing/DeviceEditsTray';
import { deviceTabSaving, findDeviceEdit, forgetDeviceTab, retainedDeviceEntry, saveDeviceEdit, useDeviceEdits } from '../features/device-editing/store';
import { StartupImportNotice } from '../features/profile-import/StartupImportNotice';
import type { SettingsSection } from '../features/settings/contracts';
import './shell.css';

type WorkspaceSwitchResume = CatalogEditResume;

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

async function boundedPersistenceDrain(pending: Promise<unknown>): Promise<void> {
  await Promise.race([
    pending,
    new Promise<void>((resolve) => {
      window.setTimeout(resolve, 2000);
    }),
  ]);
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
  const focusMode = useSettingsStore((state) => state.focusMode);

  // --- local UI state ---------------------------------------------------
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [catalogObservation, setCatalogObservation] = useState(0);
  // Which mode the palette opens in: Cmd+K search vs Cmd+P file jump (one
  // component, two modes — the P4 unification).
  const [paletteMode, setPaletteMode] = useState<'search' | 'file'>('file');
  const [sheetOpen, setSheetOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>('appearance');
  const [returnController, setReturnController] = useState(false);
  const [resumeReturnOnOpen, setResumeReturnOnOpen] = useState(false);
  const [providerDocument, setProviderDocument] = useState<ProviderReaderReference | null>(null);
  const [selectedProviderSources, setSelectedProviderSources] = useState<string[]>([]);
  const [providerRefreshKey, setProviderRefreshKey] = useState(0);

  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void readerBackend().app
      .onShowSettings(() => setSettingsOpen(true))
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
  const canonicalHistory = useCanonicalHistory();
  const { reference: canonicalDocument, setReference: setCanonicalDocument } = canonicalHistory;
  const providerEditing = useProviderEditing(canonicalDocument, canonicalHistory.tab?.id, toast, providerRefreshKey);
  const readOnlyRoot = useReadOnlyRoot(root, providerRefreshKey);
  const readOnlyRootRef = useRef(readOnlyRoot);
  readOnlyRootRef.current = readOnlyRoot;
  useCurrentProviderReopen(canonicalDocument, canonicalHistory.tab?.id, toast);
  const deviceEditing = useDeviceEditing(canonicalDocument, canonicalHistory.tab?.id, toast);
  const archivedReviews = useArchivedReviewSelection(canonicalDocument);
  const archivedReviewTarget = archivedReviews.target;
  const [archiveDocument, setArchiveDocument] = useState<{ reference: ProviderReaderReference; kind: string | null } | null>(null);
  const [semanticWorkspaceGeneration, setSemanticWorkspaceGeneration] = useState(0);
  const clearLibraryDocument = useCallback(() => setCanonicalDocument(null), [setCanonicalDocument]);
  const canonicalTabIntent = useCallback(() => {}, []);
  const refreshLibrary = useCallback(() => setProviderRefreshKey((value) => value + 1), []);
  const tabCycle = useMemo(() => createTabCycle(clearLibraryDocument, canonicalTabIntent), [clearLibraryDocument, canonicalTabIntent]);
  const [reloadNonce, setReloadNonce] = useState(0);
  useEffect(() => {
    function changed(event: Event) {
      refreshLibrary();
      const receipt = (event as CustomEvent<{ documentId?: string; sourceId?: string; revision?: string }>).detail;
      if (receipt && canonicalDocument && isDeviceReference(canonicalDocument)
        && receipt.documentId === canonicalDocument.documentId && receipt.sourceId === canonicalDocument.sourceId
        && receipt.revision === canonicalDocument.revision) setReloadNonce((value) => value + 1);
    }
    window.addEventListener(DEVICE_READ_CHANGED, changed);
    return () => window.removeEventListener(DEVICE_READ_CHANGED, changed);
  }, [canonicalDocument, refreshLibrary]);
  const [docWords, setDocWords] = useState<number | null>(null);
  const [hydrated, setHydrated] = useState(false);
  const [persistenceReady, setPersistenceReady] = useState(false);
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
        resume?: WorkspaceSwitchResume;
      }
    | {
        kind: 'forget';
        source: LibrarySourceDto;
        expectedSourceIntent: number;
      }
    | null
  >(null);
  // A pending provenance jump handed to the ReaderPane once the target renders.
  // It carries the WHOLE range, not just where to land: a citation that names
  // lines 41–47 must arrive at the reader still naming 41–47, or the reader
  // cannot mark what was actually cited. See `JumpTarget`.
  const [jump, setJump] = useState<JumpTarget | null>(null);
  const jumpSeq = useRef(0);

  const openCatalogDocument = useCallback((reference: ProviderReaderReference, intent: 'reopen' | 'exact' = 'reopen') => {
    setCanonicalDocument(reference, intent);
    setPaletteOpen(false);
  }, [setCanonicalDocument]);

  const openCreatedReceipt = useCallback((reference: ProviderReaderReference) => {
    setCanonicalDocument(reference, 'exact');
    setPaletteOpen(false);
  }, [setCanonicalDocument]);
  const openCreatedDocument = useCreatedProviderEdit(canonicalDocument, providerEditing.canEdit, providerEditing.editValidated, openCreatedReceipt, refreshLibrary, toast);

  const openProviderDocument = useCallback((reference: ProviderReaderReference, intent: 'reopen' | 'exact' = 'reopen') => {
    setCanonicalDocument(reference, intent);
    setSettingsOpen(false);
    setPaletteOpen(false);
  }, [setCanonicalDocument]);

  const activePathRef = useRef<string | null>(null);
  activePathRef.current = activeTab && isLocalDocument(activeTab.docRef) ? activeTab.docRef.path : null;
  const activeTabIdRef = useRef<string | null>(null);
  activeTabIdRef.current = activeTabId;
  // Late-bound so the close-guard's `performQuit` (declared before `flushSession`)
  // can reach the latest flusher without a declaration-order dependency.
  const flushSessionRef = useRef<() => void>(() => {});
  // Late-bound so `proceedAfterGuard` (declared before `commitSwitch`) can reach it.
  const commitSwitchRef = useRef<
    (folder: string, sourceId?: string, expectedSourceIntent?: number) => Promise<boolean>
  >(() => Promise.resolve(false));
  const commitForgetRef = useRef<
    (source: LibrarySourceDto, expectedSourceIntent: number) => Promise<void>
  >(() => Promise.resolve());
  const editCatalogRef = useRef<(reference: ProviderReaderReference, intent: number, selection: number) => Promise<void>>(() => Promise.resolve());
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
      setCatalogObservation(value => value + 1);
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
      setCatalogObservation(value => value + 1);
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
          // The grant is live and the restore preference is committed. Supersede
          // every pre-activation hydration with a new backend read; renderer
          // evidence must come from that post-commit snapshot, never from an
          // optimistic membership.active rewrite.
          void refreshLibrary();
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
    [openRoot, refreshLibrary],
  );
  commitSwitchRef.current = commitSwitch;

  const commitForget = useCallback(
    (source: LibrarySourceDto, expectedSourceIntent: number): Promise<void> => {
      const run = async () => {
        if (sourceIntentRef.current !== expectedSourceIntent) return;
        const closingRoot = useSourcesStore.getState().root;
        if (closingRoot !== source.path || activeSourceIdRef.current !== source.id) {
          toast('That source is no longer the active workspace');
          return;
        }
        let outcome;
        try {
          outcome = await closeAndForgetLibrarySource(source.id);
        } catch {
          toast('Could not close and forget this source');
          return;
        }

        // Native success means the exact epoch and watcher are already gone.
        // Clear every renderer-held echo of that authority before best-effort
        // library hydration or any later queued source operation.
        openEpochRef.current += 1;
        watcherGenRef.current = -1;
        activeSourceIdRef.current = null;
        setSemanticWorkspaceGeneration(outcome.workspace_generation);
        setCanonicalDocument(null);
        useTabsStore.getState().closeAll();
        useEditorStore.getState().retain((tabId, path) => retainedProviderEntry(tabId, path) || retainedDeviceEntry(tabId, path));
        useSourcesStore.getState().forgetRoot(closingRoot);
        flushSessionRef.current();
        await boundedPersistenceDrain(sessionWriter.drain());

        if (outcome.status.kind === 'cleanup-pending') {
          toast('Source closed. MDReader will retry private index cleanup.');
        } else if (outcome.status.kind === 'inactive') {
          toast('Source closed, but its private index could not be removed. Retry Forget.');
        }
        void refreshLibrary();
      };
      const result = switchQueueRef.current.then(run, run);
      switchQueueRef.current = result.then(
        () => undefined,
        () => undefined,
      );
      return result;
    },
    [refreshLibrary],
  );
  commitForgetRef.current = commitForget;

  const forgetActiveSource = useCallback(
    async (source: LibrarySourceDto) => {
      const expectedSourceIntent = beginSourceIntent();
      if (useEditorStore.getState().dirtyTabIds().length > 0) {
        setCloseGuard({ kind: 'forget', source, expectedSourceIntent });
        return;
      }
      await commitForget(source, expectedSourceIntent);
    },
    [beginSourceIntent, commitForget],
  );

  // Switch the active workspace to `folder`. A real switch (a different root)
  // retires the previous workspace's tabs, which would tear down any unsaved edit
  // — so if ANY tab is dirty, route through the unsaved-changes guard first
  // (Save-all / Discard / Cancel) and let it commit the switch. A clean workspace
  // (or a same-root supersede) switches immediately.
  const switchWorkspace = useCallback(
    (
      folder: string,
      sourceId?: string,
      resume?: WorkspaceSwitchResume,
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

  const openRecentDocument = useCallback(
    (ref: DocumentRef) => {
      if (isLocalDocument(ref)) {
        openRef(ref);
        return;
      }
      clearLibraryDocument();
      useTabsStore.getState().openTab(ref, { originRoot: null });
    },
    [openRef, clearLibraryDocument],
  );

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
    if (!isLocalDocument(tab.docRef)) return;
    if (mode !== 'read' && readOnlyRootRef.current) { toast('This source is read-only'); return; }
    const docPath = tab.docRef.path;
    const editor = useEditorStore.getState();
    if (mode === 'read') {
      try {
        await keepDraftForReading(tabId, docPath, () => {
          const selected = selectActiveTab(useTabsStore.getState());
          return sourceIntentIsCurrent() && selected?.id === tabId && isLocalDocument(selected.docRef) && selected.docRef.path === docPath;
        }, () => useEditorStore.getState().setMode(tabId, docPath, 'read'));
      } catch { toast('Your draft could not be kept. Retry or save a copy before leaving.'); }
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
      if (!landed || !isLocalDocument(landed.docRef) || landed.id !== tabId || landed.docRef.path !== docPath) return;
      useEditorStore.getState().seed(tabId, docPath, doc.content, doc.hash, mode);
    } catch (cause) {
      if (sourceIntentIsCurrent()) {
        toast(`Could not open for editing — ${cause instanceof Error ? cause.message : String(cause)}`);
      }
    }
  }, []);

  const catalogEditing = useCatalogEditing({
    beginIntent: beginSourceIntent,
    isIntentCurrent: (intent) => sourceIntentRef.current === intent,
    switchWorkspace,
    openLocal: (path, name) => openRef({ path, name }),
    notify: toast,
  });
  editCatalogRef.current = catalogEditing.edit;

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
    try {
      const reference = providerTarget(target);
      if (reference) {
        if (opts.background) { toast('Service-owned results currently open in the foreground. Press Enter to read this exact revision.'); return; }
        openCatalogDocument(reference, 'exact');
        return;
      }
    } catch (cause) { toast(cause instanceof Error ? cause.message : 'The cloud result is unavailable.'); return; }
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
  }, [clearLibraryDocument, openCatalogDocument]);

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

  const focusOutline = useCallback(() => {
    useSettingsStore.setState({ outlineVisible: true });
    requestAnimationFrame(() => {
      document.querySelector<HTMLElement>('.outline-rail a')?.focus();
    });
  }, []);

  // --- light editing (source/split, hash-guarded save, conflicts) --------
  // Cmd+E: toggle read ↔ source for the active tab's current doc.
  const toggleSourceMode = useCallback(() => {
    if (readerActive()) {
      if (canonicalDocument && isDeviceReference(canonicalDocument)) {
        if (deviceEditing.activeKey) deviceEditing.read();
        else void deviceEditing.edit(canonicalDocument);
        return;
      }
      if (providerEditing.activeKey) providerEditing.read();
      else if (canonicalDocument?.sourceId.startsWith('provider:')) void providerEditing.edit(canonicalDocument);
      return;
    }
    const tab = selectActiveTab(useTabsStore.getState());
    if (!tab) return;
    const current = useEditorStore.getState().mode(tab.id, documentKey(tab.docRef));
    void setEditorMode(current === 'read' ? 'source' : 'read');
  }, [setEditorMode, providerEditing, deviceEditing, canonicalDocument]);

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
      const save = useEditorStore.getState().beginSave(tabId, path);
      if (!save) { toast('Editing is paused or this document is already saving.'); return 'error'; }
      try {
        const captured = save.entry;
        const result = await runSave({
          path: captured.path, buffer: captured.buffer, baseHash: captured.baseHash,
          force, save: saveDoc,
        });
        if (result.kind === 'saved') {
          void triggerReindex();
          if (!save.saved(result.newHash)) {
            toast('Save completed for an earlier editor state. Your current buffer was kept.');
            return 'error';
          }
          // Our watcher echo is deliberately suppressed after reanchoring the
          // editor. Refresh the saved reader explicitly so Done shows the body
          // just confirmed on disk, without publishing newer unsaved edits.
          const active = selectActiveTab(useTabsStore.getState());
          if (!readerActive() && active?.id === tabId && isLocalDocument(active.docRef) && active.docRef.path === path)
            setReloadNonce(value => value + 1);
          return 'saved';
        }
        if (result.kind === 'conflict') {
          save.conflicted(result.currentHash);
          return 'conflict';
        }
        toast(`Could not save — ${result.message}`);
        return 'error';
      } finally { save.finish(); }
    },
    [triggerReindex],
  );

  // Save the active tab's CURRENT document (Cmd+S / the editor's Cmd+S). If the
  // tab has navigated to a doc with no entry, this is a no-op — a buffer for a doc
  // the tab left is never written to the doc it now shows.
  const saveActiveDoc = useCallback(
    (force: boolean) => {
      if (readerActive()) {
        const id = useReaderHistoryStore.getState().data.activeTabId;
        if (id && canonicalDocument && isDeviceReference(canonicalDocument)) {
          const edit = findDeviceEdit(`reader:${id}`, canonicalDocument);
          if (!edit) return Promise.resolve('noop' as const);
          const selection = useReaderHistoryStore.getState().selection;
          return saveDeviceEdit(edit.key).then((outcome) => {
            if (outcome === 'saved') deviceEditing.published(edit.key, selection);
            return outcome;
          });
        }
        const edit = id && canonicalDocument ? findProviderEdit(`reader:${id}`, canonicalDocument) : null;
        if (!edit) return Promise.resolve('noop' as const);
        const selection = useReaderHistoryStore.getState().selection;
        return saveProviderEdit(edit.key).then((outcome) => {
          if (outcome === 'saved') { providerEditing.published(edit.key, selection); refreshLibrary(); }
          return outcome;
        });
      }
      const tab = selectActiveTab(useTabsStore.getState());
      if (!tab) return Promise.resolve('noop' as const);
      if (!isLocalDocument(tab.docRef)) return Promise.resolve('noop' as const);
      return saveDocEntry(tab.id, tab.docRef.path, force);
    },
    [saveDocEntry, canonicalDocument, deviceEditing, providerEditing, refreshLibrary],
  );

  // Historical cloud buffers cannot be saved through a retired runtime. Keep
  // them dirty and available for explicit recovery instead of treating a key as
  // a filesystem path or manufacturing a successful save.
  const saveCloudEntry = useCallback(async (key: string): Promise<'saved' | 'error' | 'noop'> => {
    if (useProviderEdits.getState().edits[key]) return saveProviderEdit(key);
    toast('This historical edit needs offline migration. The unsaved buffer was kept.');
    return 'error';
  }, []);

  // Conflict banner "Reload": discard the local buffer and adopt the on-disk
  // version (re-seeding the anchor), and refresh the reader view too.
  const reloadActiveDoc = useCallback(async () => {
    const tab = selectActiveTab(useTabsStore.getState());
    if (!tab) return;
    const tabId = tab.id;
    if (!isLocalDocument(tab.docRef)) return;
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
  const closeTabAndBuffer = useCallback(async (tabId: string) => {
    if (editorAdmission.denied()) { toast('Preparation is in progress. Your tab and buffers were kept.'); return; }
    if (providerTabSaving(tabId)) { toast('This cloud save is still running. Wait for its outcome before closing.'); return; }
    if (deviceTabSaving(tabId)) { toast('Check the pending Markdown save before closing this tab. Your draft was kept.'); return; }
    const entries = Object.values(useEditorStore.getState().entries).filter(entry => entry.tabId === tabId);
    try { await retireEditorDrafts(entries); }
    catch { toast('The tab could not be closed safely. Your draft is kept.'); return; }
    if (editorAdmission.denied() || entries.some(entry => useEditorStore.getState().entry(entry.tabId, entry.path) !== entry)) { resumeEditorDrafts(entries); return; }
    useEditorStore.getState().removeTab(tabId);
    if (Object.values(useEditorStore.getState().entries).some((entry) => entry.tabId === tabId)) {
      toast('Editing is paused. The tab and its buffers were kept.'); return;
    }
    forgetProviderTab(tabId);
    forgetDeviceTab(tabId);
    if (tabId.startsWith('reader:')) useReaderHistoryStore.getState().close(tabId.slice(7));
    else { if (!readerActive()) clearLibraryDocument(); useTabsStore.getState().closeTab(tabId); }
  }, [clearLibraryDocument]);

  // Request a tab close: a tab with ANY unsaved document prompts the guard first;
  // a clean one closes immediately. Every close path (⌘W, the tab's ×, middle
  // click) routes through here, so a dirty buffer is never silently discarded.
  const requestCloseTab = useCallback(
    (tabId: string) => {
      if (!readerActive()) clearLibraryDocument();
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
        const kind = useDeviceEdits.getState().edits[entry.path] ? await saveDeviceEdit(entry.path) : entry.path.startsWith('cloud:')
          ? await saveCloudEntry(entry.path)
          : await saveDocEntry(entry.tabId, entry.path, false);
        if (kind !== 'saved' && kind !== 'noop') allClean = false;
      }
      return allClean;
    },
    [saveCloudEntry, saveDocEntry],
  );

  // Save every dirty document across all tabs (Save on the quit / switch guard).
  const saveAllDirty = useCallback(async (): Promise<boolean> => {
    let allClean = true;
    for (const entry of useEditorStore.getState().allDirtyEntries()) {
      const kind = useDeviceEdits.getState().edits[entry.path] ? await saveDeviceEdit(entry.path) : entry.path.startsWith('cloud:')
        ? await saveCloudEntry(entry.path)
        : await saveDocEntry(entry.tabId, entry.path, false);
      if (kind !== 'saved' && kind !== 'noop') allClean = false;
    }
    return allClean;
  }, [saveCloudEntry, saveDocEntry]);

  // Complete a quit the guard approved (Save-then-close or Discard). The session
  // was already drained in the close handler before the prompt; here we clear any
  // remaining dirty state and force the window down. We use `destroy()` (not
  // `close()`): the window was already `preventDefault`'d in onCloseRequested, and
  // `close()` merely re-requests a close the webview won't honour; `destroy()`
  // tears it down unconditionally.
  const performQuit = useCallback(async () => {
    if (editorAdmission.denied()) { toast('Preparation is in progress. Keep this window open.'); return; }
    const entries = Object.values(useEditorStore.getState().entries);
    try { await retireEditorDrafts(entries); }
    catch { toast('Draft cleanup could not be confirmed. Keep this window open.'); return; }
    if (editorAdmission.denied() || entries.some(entry => useEditorStore.getState().entry(entry.tabId, entry.path) !== entry)) { resumeEditorDrafts(entries); return; }
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
            resume?: WorkspaceSwitchResume;
          }
        | {
            kind: 'forget';
            source: LibrarySourceDto;
            expectedSourceIntent: number;
          },
    ) => {
      if (editorAdmission.denied()) { toast('Preparation is in progress. Your buffers were kept.'); return; }
      if (guard.kind === 'tab') closeTabAndBuffer(guard.tabId);
      else if (guard.kind === 'quit') void performQuit();
      else if (guard.kind === 'forget') {
        void commitForgetRef.current(guard.source, guard.expectedSourceIntent);
      } else {
        void (async () => {
          if (guard.resume?.kind === 'catalog-edit' && useReaderHistoryStore.getState().selection !== guard.resume.selection) return;
          const committed = await commitSwitchRef.current(
            guard.folder,
            guard.sourceId,
            guard.expectedSourceIntent,
          );
          if (committed && guard.resume?.kind === 'catalog-edit' && sourceIntentRef.current === guard.expectedSourceIntent) {
            await editCatalogRef.current(guard.resume.reference, guard.expectedSourceIntent, guard.resume.selection);
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
    if (allClean && !stillDirty) {
      try { await flushEditorDrafts(); }
      catch { toast('The saved draft could not be closed safely. Your tab is kept.'); return; }
      if (closeGuardRef.current !== guard || (guard.kind === 'tab'
        ? useEditorStore.getState().tabIsDirty(guard.tabId)
        : useEditorStore.getState().dirtyTabIds().length > 0)) return;
      setCloseGuard(null); proceedAfterGuard(guard);
    } else setCloseGuard(null);
  }, [saveTabAll, saveAllDirty, proceedAfterGuard]);

  // Guard "Discard": proceed, losing the unsaved edits.
  const onGuardDiscard = useCallback(async () => {
    const guard = closeGuardRef.current;
    if (!guard) return;
    const entries = Object.values(useEditorStore.getState().entries).filter(entry => guard.kind !== 'tab' || entry.tabId === guard.tabId);
    try { await retireEditorDrafts(entries); }
    catch { toast('The discard could not be confirmed. Your tabs and drafts were kept.'); return; }
    if (closeGuardRef.current !== guard || editorAdmission.denied()
      || entries.some(entry => useEditorStore.getState().entry(entry.tabId, entry.path) !== entry)) { resumeEditorDrafts(entries); return; }
    setCloseGuard(null);
    proceedAfterGuard(guard);
  }, [proceedAfterGuard]);

  // Bound editor memory. Keep an entry only when it is DIRTY (never drop an
  // unsaved edit) OR it is a live tab's currently-active document. This reclaims
  // entries for closed/retired tabs (already guard-handled if they were dirty) AND
  // evicts clean buffers for docs a tab has navigated away from — so a long-lived
  // tab that has viewed many docs in source mode does not grow unbounded.
  useEffect(() => {
    // Local editor entries use the native path, while tab identities carry a
    // local: prefix. Compare like keys, using the live tab snapshot so a queued
    // effect cannot evict an editor seeded by a newer explicit handoff.
    const activePathByTab = new Map(useTabsStore.getState().tabs.map((tab) =>
      [tab.id, isLocalDocument(tab.docRef) ? tab.docRef.path : documentKey(tab.docRef)]));
    useEditorStore
      .getState()
      .retain((tabId, path, dirty) => dirty || retainedProviderEntry(tabId, path) || retainedDeviceEntry(tabId, path) || activePathByTab.get(tabId) === path);
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
        allowInSourceEditor: true,
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
        allowInSourceEditor: true,
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
          const readerId = useReaderHistoryStore.getState().data.activeTabId;
          if (readerId) { requestCloseTab(`reader:${readerId}`); return; }
          const id = tabsApi().activeTabId;
          if (id) requestCloseTab(id); // dirty → unsaved-changes guard
        },
      },
      {
        id: 'reopen-tab',
        keys: 'Mod+Shift+T',
        description: 'Reopen closed tab',
        group: 'Tabs',
        run: () => {
          if (useReaderHistoryStore.getState().reopen()) canonicalTabIntent();
          else { clearLibraryDocument(); tabsApi().reopenClosedTab(); }
        },
      },
      {
        id: 'edit-source',
        keys: 'Mod+E',
        description: 'Toggle source view',
        group: 'Editing',
        run: () => toggleSourceMode(),
      },
      {
        id: 'save',
        keys: 'Mod+S',
        description: 'Save document',
        group: 'Editing',
        run: () => { void saveActiveDoc(false); },
      },
      {
        id: 'mru-next',
        keys: 'Ctrl+Tab',
        description: 'Next tab (recent)',
        group: 'Tabs',
        run: () => tabCycle.cycle(1),
      },
      {
        id: 'mru-prev',
        keys: 'Ctrl+Shift+Tab',
        description: 'Previous tab (recent)',
        group: 'Tabs',
        run: () => tabCycle.cycle(-1),
      },
      {
        id: 'back',
        keys: 'Mod+[',
        description: 'Back',
        group: 'Navigation',
        run: () => {
          if (stepReaderTab(-1)) return;
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
          if (stepReaderTab(1)) return;
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
  }, [isMac, focusOutline, toggleSourceMode, saveActiveDoc, requestCloseTab, canonicalTabIntent, clearLibraryDocument, tabCycle]);

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
      if (event.defaultPrevented) return;
      const size = textSizeChord(isMac, event);
      if (size && !paletteOpen && !sheetOpen && !settingsOpen && !archivedReviews.isOpen
        && !isEditable(document.activeElement)) {
        event.preventDefault();
        useSettingsStore.getState()[size === 'increase' ? 'increaseTextSize' : 'decreaseTextSize']();
        return;
      }
      // The native modal dialog owns Escape, focus and document shortcuts.
      if (providerDocument || archivedReviews.isOpen) return;
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
        settingsOpen;
      const inSourceEditor = !paletteOpen && !sheetOpen && !settingsOpen
        && Boolean(document.activeElement?.closest('.cm-editor'));
      if (registry.dispatch(event, { inEditable, inSourceEditor })) event.preventDefault();
    }
    function onKeyUp(event: KeyboardEvent) {
      if (event.key === 'Control') tabCycle.finish();
    }
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('keyup', onKeyUp);
    return () => {
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('keyup', onKeyUp);
    };
  }, [
    registry,
    tabCycle,
    paletteOpen,
    sheetOpen,
    settingsOpen,
    providerDocument,
    archivedReviews.isOpen,
  ]);

  // --- startup: restore session + settings ------------------------------
  useEffect(() => {
    let active = true;
    const startupIntent = sourceIntentRef.current;
    const readerChanges = useReaderHistoryStore.getState().changes;
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
      if (persisted.restoreFailed) {
        if (active) toast('Saved preferences could not be read. Changes will not be saved until MDReader can read them.', 8000);
        if (active) setHydrated(true);
        return;
      }
      const { session, settings } = persisted;
      if (Object.keys(settings).length > 0) useSettingsStore.getState().hydrate(settings);
      if (active) useReaderHistoryStore.getState().hydrate(persisted.readerHistory ?? restoreReaderHistory(undefined, false), readerChanges);
      if (!startupIsCurrent()) {
        if (active) { setPersistenceReady(true); setHydrated(true); }
        return;
      }
      if (session) {
        useTabsStore.getState().hydrate(session.tabs, session.activeTabId);
        // A serialized path is history, not a filesystem grant. Keep its Recent
        // metadata, but leave the live root empty until backend-owned membership
        // confirms this exact source still exists and is available.
        useSourcesStore.getState().hydrate({ root: null, recent: session.recent });
        if (session.root && isTauri() && !readerActive()) {
          try {
            const member = await reconcileLibraryRestore(session.root);
            if (startupIsCurrent() && member && !readerActive()) {
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
      if (active) { setPersistenceReady(true); setHydrated(true); }
    })();
    return () => {
      active = false;
    };
  }, []);

  // --- theme application ------------------------------------------------
  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  const requestQuitGuard = useCallback(() => setCloseGuard({ kind: 'quit' }), []);
  useDraftRecovery(persistenceReady);
  useAppPersistence({ hydrated, persistenceReady, flushSessionRef, closingRef, requestQuit: requestQuitGuard });

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

  // Local Recent remains scoped to its workspace grant. Cloud Recent belongs to
  // the Library and retains provider identity across local workspace switches.
  const visibleRecent = useMemo<DocumentRef[]>(
    () =>
      recent.flatMap<DocumentRef>((entry) => {
        if (entry.kind === 'cloud') {
          return [{
            kind: 'cloud' as const,
            sourceId: entry.sourceId,
            recordKey: entry.recordKey,
            documentKey: entry.documentKey,
            name: entry.name,
          }];
        }
        return entry.root === root ? [{ path: entry.path, name: entry.name }] : [];
      }),
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
    () => (activeTab && isLocalDocument(activeTab.docRef) ? { path: activeTab.docRef.path, name: activeTab.docRef.name } : null),
    [activeTab],
  );
  // The search/related transport, closed over the live watcher generation so
  // streamed batches from a superseded workspace epoch are discarded.
  const adapter = useMemo(() => makeSearchAdapter(() => root ? watcherGenRef.current : 0, () => selectedProviderSources), [root, selectedProviderSources]);
  const searchScopeKey = `${root ?? ""}|${selectedProviderSources.join(",")}`;
  const archiveReference = canonicalDocument && !isDeviceReference(canonicalDocument)
    && archiveDocument?.reference === canonicalDocument && (archiveDocument.kind === 'local' || archiveDocument.kind === 'provider')
    ? archiveDocument.reference : null;
  const archiveLocalDocument = !canonicalDocument && activeTab && isLocalDocument(activeTab.docRef) ? activeTab.docRef : null;
  const readerBlocked = Boolean(archivedReviews.isOpen || providerDocument || settingsOpen || paletteOpen || sheetOpen || closeGuard);

  return (
    <div className={`shell${focusMode ? ' shell--focus' : ''}`}>
      <div className="shell__reader-content" inert={readerBlocked} aria-hidden={readerBlocked ? true : undefined}>
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
          activePath={activeTab && isLocalDocument(activeTab.docRef) ? activeTab.docRef.path : null}
          loading={loading}
          listingError={listingError}
          onPickFolder={onPickFolder}
          providerSources={isTauri() ? (
            <><CatalogSidebar
              observationKey={catalogObservation}
              selectedProviderSources={selectedProviderSources}
              onProviderSelectionChange={setSelectedProviderSources}
              refreshKey={`${providerRefreshKey}:${semanticWorkspaceGeneration}`}
              onOpen={openCatalogDocument}
              onCreated={openCreatedDocument}
              onManage={() => { setSettingsSection('library'); setSettingsOpen(true); }}
            /><AgentSessionsPane refreshKey={settingsOpen}
              onOpen={openCatalogDocument} onManage={() => { setSettingsSection('agents'); setSettingsOpen(true); }} />
              <ProviderEditsTray onOpen={providerEditing.openDraft} /><DeviceEditsTray onOpen={deviceEditing.openDraft} /></>
          ) : undefined}
          recentContent={isTauri() ? <ReaderRecents onOpen={openCatalogDocument}
            workspaceRecent={visibleRecent} onOpenWorkspace={openRecentDocument} /> : undefined}
          onOpenFile={openFile}
          onOpenRecent={openRecentDocument}
          isIgnored={isIgnored}
        />

        <main className="shell__main">
          <StartupImportNotice onImport={() => { setSettingsSection('backup'); setSettingsOpen(true); }}
            onReturnController={() => {
              setReturnController(true); setResumeReturnOnOpen(true);
              setSettingsSection('backup'); setSettingsOpen(true);
            }} />
          <ReaderTabs
            onLocalIntent={clearLibraryDocument}
            onCanonicalIntent={canonicalTabIntent}
            onRequestClose={requestCloseTab}
          />
          <DraftRecovery key={`${canonicalHistory.tab?.id ?? activeTab?.id}:${providerEditing.activeKey ?? deviceEditing.activeKey ?? activeTab?.docRef.path ?? ''}`}
            tabId={canonicalDocument ? canonicalHistory.tab ? `reader:${canonicalHistory.tab.id}` : null : activeTab?.id ?? null}
            path={canonicalDocument ? providerEditing.activeKey ?? deviceEditing.activeKey : activeTab && isLocalDocument(activeTab.docRef) ? activeTab.docRef.path : null} />
          {canonicalDocument && !providerEditing.activeKey && !deviceEditing.activeKey && (isDeviceReference(canonicalDocument) ? deviceEditing.draftKept : providerEditing.draftKept) ? <p className="editor-draft-kept" role="status">Draft kept · Reading saved version</p> : null}
          <div
            className={`shell__reader-row${
              relatedRailOpen ? ' shell__reader-row--related-open' : ''
            }${canonicalDocument || activeTab?.docRef.kind === 'cloud' ? ' shell__reader-row--library-open' : ''
            }`}
          >
            {canonicalDocument ? (
              isDeviceReference(canonicalDocument) ? deviceEditing.activeKey ? <DeviceEditor
                key={deviceEditing.activeKey} editKey={deviceEditing.activeKey} onRead={deviceEditing.read}
                onPublished={(selection) => { if (deviceEditing.activeKey) deviceEditing.published(deviceEditing.activeKey, selection); refreshLibrary(); }}
              /> : <DeviceReader
                key={`${canonicalDocument.documentId}:${canonicalDocument.revision}:${reloadNonce}`}
                reference={canonicalDocument}
                onEdit={deviceEditing.edit} editBusy={deviceEditing.opening}
                onNavigateReference={canonicalHistory.navigate}
                position={canonicalHistory.entry?.position}
                onPosition={canonicalHistory.savePosition}
                onBack={canonicalHistory.back}
                onForward={canonicalHistory.forward}
                canGoBack={Boolean(canonicalHistory.tab && canonicalHistory.tab.index > 0)}
                canGoForward={Boolean(canonicalHistory.tab && canonicalHistory.tab.index + 1 < canonicalHistory.tab.entries.length)}
                onClose={() => { if (canonicalHistory.tab) requestCloseTab(`reader:${canonicalHistory.tab.id}`); }}
              /> : providerEditing.activeKey ? <ProviderEditor
                key={providerEditing.activeKey}
                editKey={providerEditing.activeKey}
                onRead={providerEditing.read}
                onPublished={(selection) => { if (providerEditing.activeKey) providerEditing.published(providerEditing.activeKey, selection); refreshLibrary(); }}
              /> : <CanonicalReader
                reference={canonicalDocument}
                position={canonicalHistory.entry?.position}
                onPosition={canonicalHistory.savePosition}
                onNavigateReference={canonicalHistory.navigate}
                onBack={canonicalHistory.back}
                onForward={canonicalHistory.forward}
                canGoBack={Boolean(canonicalHistory.tab && canonicalHistory.tab.index > 0)}
                canGoForward={Boolean(canonicalHistory.tab && canonicalHistory.tab.index + 1 < canonicalHistory.tab.entries.length)}
                sourceKind={canonicalDocument.sourceId.startsWith('provider:') ? 'provider' : 'local'}
                backend={canonicalDocument.sourceId.startsWith('provider:') ? undefined : catalogBackend}
                onEdit={canonicalDocument.sourceId.startsWith('provider:')
                  ? providerEditing.canEdit ? (reference) => { void providerEditing.edit(reference); } : undefined
                  : (reference) => { void catalogEditing.edit(reference); }}
                editBusy={canonicalDocument.sourceId.startsWith('provider:') ? providerEditing.opening : catalogEditing.opening === canonicalDocument}
                onVerifiedRead={(reference, kind) => setArchiveDocument({ reference, kind })}
                onClose={() => { if (canonicalHistory.tab) requestCloseTab(`reader:${canonicalHistory.tab.id}`); }}
              />
            ) : activeTab?.docRef.kind === 'cloud' ? (
              <LegacyRecordNotice reference={activeTab.docRef} tabId={activeTab.id} />
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
              onSaveActive={() => saveActiveDoc(false)}
              onReloadActive={() => void reloadActiveDoc()}
              onOverwriteActive={() => void saveActiveDoc(true)}
              readOnly={readOnlyRoot}
              />
            </>
            )}
            {canonicalDocument || activeTab ? (
              <RelatedRail
                activeDoc={canonicalDocument ? { path: canonicalDocument.path, name: canonicalDocument.title ?? canonicalDocument.path } : activeDoc}
                canonicalReference={canonicalDocument ? { sourceId: canonicalDocument.sourceId, path: canonicalDocument.path, revision: canonicalDocument.revision, deviceDocumentId: canonicalDocument.documentId } : undefined}
                adapter={adapter}
                open={relatedRailOpen}
                onToggle={() => useSettingsStore.getState().toggleRelatedRail()}
                onOpen={(target) => onOpenHit(target, { background: false })}
                workspaceKey={canonicalDocument?.sourceId ?? root}
                refreshKey={`${reloadNonce}:${catalogObservation}:${providerRefreshKey}:${semanticWorkspaceGeneration}`}
              />
            ) : null}
          </div>
        </main>
      </div>

      <StatusBar
        words={canonicalDocument ? null : docWords}
        watcher={watcher}
        fileCount={files.length}
        listingError={listingError}
        importState={importState}
        fromIndex={fromIndex}
        onRetryIndex={retryIndex}
      />
      </div>

      {paletteOpen ? (
        <SearchPalette
          mode={paletteMode}
          files={paletteFiles}
          activeDoc={activeDoc}
          sourceName={paletteMode === 'file' ? sourceName : selectedProviderSources.length ? 'Selected sources' : sourceName}
          workspaceKey={searchScopeKey}
          workspaceRoot={root}
          adapter={adapter}
          isMac={isMac}
          onOpenHit={onOpenHit}
          onAddSource={() => { setPaletteOpen(false); setSettingsSection('library'); setSettingsOpen(true); }}
          onOpenFile={onOpenPaletteFile}
          onClose={() => setPaletteOpen(false)}
        />
      ) : null}
      {sheetOpen ? (
        <ShortcutSheet groups={shortcutGroups} onClose={() => setSheetOpen(false)} />
      ) : null}
      {settingsOpen ? (
        <SettingsSheet
          initialSection={settingsSection}
          returnController={returnController} resumeReturnOnOpen={resumeReturnOnOpen}
          onReturnResumeStarted={() => setResumeReturnOnOpen(false)}
          selectedDevice={canonicalDocument && isDeviceReference(canonicalDocument) ? canonicalDocument : undefined}
          localRoot={root}
          watcher={watcher}
          importState={importState}
          onActivateSource={(source) => switchWorkspace(source.path, source.id)}
          onForgetActiveSource={forgetActiveSource}
          onSourceIntent={beginSourceIntent}
          onLibraryChanged={() => { setProviderRefreshKey((key) => key + 1); setCanonicalDocument(null); void refreshLibrary(); }}
          selectedProviderSources={selectedProviderSources} onProviderSelectionChange={setSelectedProviderSources}
          onOpenProviderDocument={async (reference) => openProviderDocument(reference)}
          onOpenReadingLink={(reference) => openProviderDocument(reference, 'exact')}
          onOpenImportedDocument={isTauri() ? openProviderDocument : undefined}
          onReconnectImportedFolder={onPickFolder}
          archivedReviewDocument={archiveReference ? archiveReference.title || archiveReference.path : archiveLocalDocument?.name}
          onOpenArchivedReviews={() => {
            if (archiveReference && canonicalDocument) archivedReviews.openCanonical(canonicalDocument, archiveReference);
            else if (archiveLocalDocument) archivedReviews.openLocal(archiveLocalDocument.path, semanticWorkspaceGeneration);
            else archivedReviews.openArchive();
            setSettingsOpen(false);
          }}
          onClose={() => setSettingsOpen(false)}
        />
      ) : null}
      {providerDocument ? (
        <ProviderReaderDialog
          reference={providerDocument}
          sourceKind={providerDocument.sourceId.startsWith('provider:') ? 'provider' : 'local'}
          backend={providerDocument.sourceId.startsWith('provider:') ? undefined : catalogBackend}
          onClose={() => {
            setProviderDocument(null);

          }}
        />
      ) : null}
      {archivedReviews.isOpen ? (
        <ArchivedReviewsPanel
          current={archivedReviewTarget}
          onClose={() => {
            const reference = archivedReviews.close();
            if (reference) setProviderDocument(reference);
          }}
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
