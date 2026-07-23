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
  onFsChanged,
  onWatchError,
  makeSearchAdapter,
  pickWorkspaceFolder,
  readDoc,
  saveDoc,
  setWorkspace,
  startImport,
  watchWorkspace,
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
import { createReindexer } from './reindex.js';
import { LoadingGate } from './loading-gate.js';
import { watcherStampCurrent } from './watcher-guard.js';
import { performSwitch } from './switch-workspace.js';
import { Titlebar } from './Titlebar.js';
import { Sidebar } from './Sidebar.js';
import { TabStrip } from './TabStrip.js';
import { ReaderPane } from './ReaderPane.js';
import { RelatedRail } from './RelatedRail.js';
import { StatusBar } from './StatusBar.js';
import { ShortcutSheet } from './ShortcutSheet.js';
import type { ShortcutGroup } from './ShortcutSheet.js';
import { UnsavedGuard } from './UnsavedGuard.js';
import { Toasts, toast } from './toasts.js';
import './shell.css';

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

  const theme = useSettingsStore((state) => state.theme);
  const readingFont = useSettingsStore((state) => state.readingFont);
  const textScaleIndex = useSettingsStore((state) => state.textScaleIndex);
  const sidebarCollapsed = useSettingsStore((state) => state.sidebarCollapsed);
  const outlineVisible = useSettingsStore((state) => state.outlineVisible);
  const focusMode = useSettingsStore((state) => state.focusMode);

  // --- local UI state ---------------------------------------------------
  const [paletteOpen, setPaletteOpen] = useState(false);
  // Which mode the palette opens in: Cmd+K search vs Cmd+P file jump (one
  // component, two modes — the P4 unification).
  const [paletteMode, setPaletteMode] = useState<'search' | 'file'>('file');
  const [sheetOpen, setSheetOpen] = useState(false);
  const [reloadNonce, setReloadNonce] = useState(0);
  const [docWords, setDocWords] = useState<number | null>(null);
  const [hydrated, setHydrated] = useState(false);
  const [relatedOpen, setRelatedOpen] = useState(false);
  // The unsaved-changes guard: closing a dirty tab, or quitting with dirty tabs,
  // routes through this prompt before anything is discarded.
  const [closeGuard, setCloseGuard] = useState<
    { kind: 'tab'; tabId: string } | { kind: 'quit' } | null
  >(null);
  // A pending provenance jump handed to the ReaderPane once the target renders.
  const [jump, setJump] = useState<{ path: string; line: number; nonce: number } | null>(null);
  const jumpSeq = useRef(0);

  const activePathRef = useRef<string | null>(null);
  activePathRef.current = activeTab?.docRef.path ?? null;
  const activeTabIdRef = useRef<string | null>(null);
  activeTabIdRef.current = activeTabId;
  // Late-bound so the close-guard's `performQuit` (declared before `flushSession`)
  // can reach the latest flusher without a declaration-order dependency.
  const flushSessionRef = useRef<() => void>(() => {});

  // --- workspace loading ------------------------------------------------
  // Frontend request generation, the UI-side mirror of the backend epoch: every
  // `openRoot` bumps it, and every async continuation (listing, import) captures
  // the value at dispatch and writes to the store ONLY while it is still current.
  // So a late continuation for an older root can never overwrite newer state.
  const openEpochRef = useRef(0);
  // The backend generation the CURRENT watcher was armed under (from
  // watchWorkspace). Watcher events/errors carry the generation they were armed
  // under; we discard mismatches so a previous watcher's still-draining events
  // can't drive — or fail — the new workspace. `-1` = no watcher yet.
  const watcherGenRef = useRef(-1);
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
    } catch {
      /* non-Tauri or unreadable — leave the list as-is */
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
        }
      } catch {
        if (openEpochRef.current === epoch) {
          sources.setFiles([]);
          sources.setFromIndex(false);
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
    try {
      const final = await startImport(folder, (progress) => {
        if (openEpochRef.current === epoch) sources.setImport(progress);
      });
      if (openEpochRef.current !== epoch) return;
      sources.setImport(final);
    } catch (cause) {
      if (openEpochRef.current !== epoch) return;
      sources.setImport({
        phase: 'error',
        filesDone: 0,
        filesTotal: 0,
        chunks: 0,
        error: cause instanceof Error ? cause.message : String(cause),
      });
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
  // reflects it as the active root. In a Tauri shell the watcher is armed FIRST:
  // watch_workspace canonicalizes the folder, arms the watcher, and adopts it as
  // the granted root — atomically — so a switch that fails to arm leaves the
  // previous grant + watcher intact and we stay on the old workspace (no
  // half-switch, no backend divergence). A FIRST open (or a re-open of the same
  // root) whose watch fails still degrades gracefully: the folder is granted
  // independently and read, just without live-reload. Never closes tabs — a
  // workspace *switch* is the caller's concern (see `switchWorkspace`), so
  // session restore reuses this without disturbing the session it just hydrated.
  const openRoot = useCallback(
    async (folder: string): Promise<boolean> => {
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
        sources.setWatcher('idle');
        await listInto(folder, epoch);
        // A newer open superseded this one during listInto → report NOT-current so
        // its (stale) success can't authorize retiring the newer workspace's tabs.
        return openEpochRef.current === epoch;
      }

      let grant: { root: string; generation: number };
      try {
        // The ONE atomic switch: arms the watcher, adopts `folder` as the grant
        // (advancing the epoch), and returns the canonical root + the generation
        // the watcher was armed under — no separate set_workspace to sequence.
        grant = await watchWorkspace(folder);
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
        let g = folder;
        try {
          g = await setWorkspace(folder);
        } catch {
          /* non-Tauri edge — keep the raw path */
        }
        // A newer open superseded this one during the await → abandon its store
        // mutations (the newer open owns the workspace now).
        if (openEpochRef.current !== epoch) return false;
        sources.setRoot(g);
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
      sources.setRoot(grant.root);
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

  // Switch the active workspace to `folder`, retiring the previous workspace's
  // open tabs into (its own) Recent — but only once the new root is actually
  // live. A switch that fails to commit (watcher could not arm) retires nothing,
  // so the user stays put with tabs intact. Retired tabs carry their originating
  // root, so they resurface only under that workspace (never as an error tab
  // reopened under the new grant). Resolves to whether the switch committed.
  const switchWorkspace = useCallback(
    (folder: string): Promise<boolean> => {
      const sources = useSourcesStore.getState();
      // `performSwitch` owns the two guarantees (same-root supersede; per-origin
      // retirement against the post-commit root) — see switch-workspace.ts.
      return performSwitch(folder, {
        openRoot,
        previousRoot: sources.root,
        currentRoot: () => useSourcesStore.getState().root,
        retireForeign: (root) => useTabsStore.getState().retireForeign(root),
        addRecent: (ref, root) => sources.addRecent(ref, root),
      });
    },
    [openRoot],
  );

  const onPickFolder = useCallback(async () => {
    let picked: string | null;
    try {
      picked = await pickWorkspaceFolder();
    } catch {
      toast('Could not open the folder picker');
      return;
    }
    if (!picked) return;
    await switchWorkspace(picked);
  }, [switchWorkspace]);

  // Re-run the import for the current workspace (e.g. after a model-load
  // timeout) — the backend re-enters the loading phase since no engine was
  // installed. Rides the current open generation.
  const retryIndex = useCallback(() => {
    void triggerReindex();
  }, [triggerReindex]);

  // --- open / navigate helpers -----------------------------------------
  const openRef = useCallback((ref: DocRef) => {
    const root = useSourcesStore.getState().root;
    useTabsStore.getState().openTab(ref, { originRoot: root });
    useSourcesStore.getState().addRecent(ref, root);
  }, []);

  const openFile = useCallback(
    (file: WorkspaceFile) => {
      openRef({ path: file.path, name: file.name });
      setPaletteOpen(false);
    },
    [openRef],
  );

  // The palette's file open (Cmd+P mode): foreground focuses the tab; ⌘↵ opens
  // it in the background (the palette itself decides when to close).
  const onOpenPaletteFile = useCallback((file: PaletteFile, opts: { background: boolean }) => {
    const root = useSourcesStore.getState().root;
    const ref = { path: file.path, name: file.name };
    useTabsStore.getState().openTab(ref, { activate: !opts.background, originRoot: root });
    useSourcesStore.getState().addRecent(ref, root);
  }, []);

  // The palette's search-hit open: land at the hit's provenance line. Enter
  // focuses the tab and schedules the jump (consumed once the doc renders); ⌘↵
  // opens a background tab (no jump — it isn't the focused reader).
  const onOpenHit = useCallback((target: HitTarget, opts: { background: boolean }) => {
    const root = useSourcesStore.getState().root;
    const ref = { path: target.path, name: target.name };
    useTabsStore.getState().openTab(ref, { activate: !opts.background, originRoot: root });
    useSourcesStore.getState().addRecent(ref, root);
    if (!opts.background && target.line != null) {
      setJump({ path: target.path, line: target.line, nonce: (jumpSeq.current += 1) });
    }
  }, []);

  const onNavigate = useCallback((ref: DocRef) => {
    const root = useSourcesStore.getState().root;
    const active = useTabsStore.getState().activeTabId;
    if (!active) {
      useTabsStore.getState().openTab(ref, { originRoot: root });
    } else {
      useTabsStore.getState().navigateInTab(active, ref);
    }
    useSourcesStore.getState().addRecent(ref, root);
  }, []);

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
  // Switch the active tab's view mode. Entering source/split for the first time
  // seeds the editor buffer from a fresh disk read (content + hash) so the save
  // guard is anchored to exactly what was on disk when editing began.
  const setEditorMode = useCallback(async (mode: EditorMode) => {
    const tabsState = useTabsStore.getState();
    const tabId = tabsState.activeTabId;
    const tab = selectActiveTab(tabsState);
    if (!tabId || !tab) return;
    const editor = useEditorStore.getState();
    if (mode === 'read') {
      if (editor.entry(tabId)) editor.setMode(tabId, 'read');
      return;
    }
    if (editor.entry(tabId)) {
      editor.setMode(tabId, mode);
      return;
    }
    const docPath = tab.docRef.path;
    try {
      const doc = await readDoc(docPath);
      // The active tab may have changed (or navigated) during the read — seed only
      // if it still stands on the same document.
      const landed = selectActiveTab(useTabsStore.getState());
      if (!landed || landed.id !== tabId || landed.docRef.path !== docPath) return;
      useEditorStore.getState().seed(tabId, docPath, doc.content, doc.hash, mode);
    } catch (cause) {
      toast(`Could not open for editing — ${cause instanceof Error ? cause.message : String(cause)}`);
    }
  }, []);

  // Cmd+E: toggle read ↔ source for the active tab (from within the editor the
  // SourcePane handles its own Cmd+E, since the editable target hides it from the
  // global handler).
  const toggleSourceMode = useCallback(() => {
    const tabId = useTabsStore.getState().activeTabId;
    if (!tabId) return;
    const current = useEditorStore.getState().mode(tabId);
    void setEditorMode(current === 'read' ? 'source' : 'read');
  }, [setEditorMode]);

  // Save tab `tabId` under the hash guard (or forced, from the conflict banner's
  // explicit overwrite). A `saved` outcome re-anchors the buffer to the fresh hash
  // — which the watcher then recognises as our own write — and queues a reindex; a
  // `conflict` raises the banner rather than clobbering the on-disk change.
  // Resolves to what happened so the close/quit guard can decide whether to
  // proceed. Works on ANY tab (not just the active one) for save-all-on-quit.
  const saveTab = useCallback(
    async (tabId: string | null, force: boolean): Promise<'saved' | 'conflict' | 'error' | 'noop'> => {
      if (!tabId) return 'noop';
      const tab = useTabsStore.getState().tabs.find((candidate) => candidate.id === tabId);
      if (!tab) return 'noop';
      const entry = useEditorStore.getState().entry(tabId);
      if (!entry) return 'noop';
      // The buffer belongs to the document it was seeded for; if the tab has since
      // navigated elsewhere, refuse — never write this buffer to another file.
      if (entry.path !== tab.docRef.path) return 'noop';
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
      // Transitions are keyed by tab id, so they land on the right tab even if the
      // active tab changed during the await.
      if (result.kind === 'saved') {
        editor.markSaved(tabId, snapshot, result.newHash);
        void triggerReindex();
        return 'saved';
      }
      if (result.kind === 'conflict') {
        editor.raiseConflict(tabId, result.currentHash, 'save');
        return 'conflict';
      }
      toast(`Could not save — ${result.message}`);
      return 'error';
    },
    [triggerReindex],
  );

  const saveActiveDoc = useCallback(
    (force: boolean) => saveTab(useTabsStore.getState().activeTabId, force),
    [saveTab],
  );

  // Conflict banner "Reload": discard the local buffer and adopt the on-disk
  // version (re-seeding the anchor), and refresh the reader view too.
  const reloadActiveDoc = useCallback(async () => {
    const tabsState = useTabsStore.getState();
    const tabId = tabsState.activeTabId;
    const tab = selectActiveTab(tabsState);
    if (!tabId || !tab) return;
    const docPath = tab.docRef.path;
    try {
      const doc = await readDoc(docPath);
      const editor = useEditorStore.getState();
      const mode = editor.mode(tabId);
      editor.seed(tabId, docPath, doc.content, doc.hash, mode === 'read' ? 'source' : mode);
      setReloadNonce((nonce) => nonce + 1);
    } catch (cause) {
      toast(`Could not reload — ${cause instanceof Error ? cause.message : String(cause)}`);
    }
  }, []);

  // Close a tab, dropping its editor buffer. The single place a tab is torn down.
  const closeTabAndBuffer = useCallback((tabId: string) => {
    useEditorStore.getState().remove(tabId);
    useTabsStore.getState().closeTab(tabId);
  }, []);

  // Request a tab close: a dirty tab prompts the unsaved-changes guard first; a
  // clean one closes immediately. Every close path (⌘W, the tab's × , middle
  // click) routes through here, so a dirty buffer is never silently discarded.
  const requestCloseTab = useCallback(
    (tabId: string) => {
      if (useEditorStore.getState().isDirty(tabId)) setCloseGuard({ kind: 'tab', tabId });
      else closeTabAndBuffer(tabId);
    },
    [closeTabAndBuffer],
  );

  // Save every dirty tab (for "Save" on the quit guard). Resolves to whether they
  // ALL landed cleanly — a conflict leaves that tab open with its banner, so the
  // quit is aborted rather than losing the buffer.
  const saveAllDirty = useCallback(async (): Promise<boolean> => {
    let allClean = true;
    for (const id of useEditorStore.getState().dirtyTabIds()) {
      const kind = await saveTab(id, false);
      if (kind !== 'saved' && kind !== 'noop') allClean = false;
    }
    return allClean;
  }, [saveTab]);

  // Persist the latest session and destroy the window for real (the actual quit).
  const performQuit = useCallback(async () => {
    try {
      flushSessionRef.current();
      await sessionWriter.drain();
    } finally {
      try {
        await getCurrentWindow().destroy();
      } catch {
        /* not a real Tauri window (mocked IPC) — nothing to destroy */
      }
    }
  }, []);

  // Guard "Save": save then proceed (close the tab / quit) — unless a conflict
  // intervened, in which case stay so the banner can be resolved.
  const onGuardSave = useCallback(async () => {
    const guard = closeGuard;
    if (!guard) return;
    if (guard.kind === 'tab') {
      const kind = await saveTab(guard.tabId, false);
      setCloseGuard(null);
      if (kind === 'saved' || kind === 'noop') closeTabAndBuffer(guard.tabId);
    } else {
      const allClean = await saveAllDirty();
      setCloseGuard(null);
      if (allClean) void performQuit();
    }
  }, [closeGuard, saveTab, saveAllDirty, performQuit, closeTabAndBuffer]);

  // Guard "Discard": proceed, losing the unsaved edits.
  const onGuardDiscard = useCallback(() => {
    const guard = closeGuard;
    if (!guard) return;
    setCloseGuard(null);
    if (guard.kind === 'tab') closeTabAndBuffer(guard.tabId);
    else void performQuit();
  }, [closeGuard, performQuit, closeTabAndBuffer]);

  // Prune editor entries that no longer belong: tabs that have closed, and tabs
  // that have NAVIGATED to a different document (the buffer was seeded for the old
  // doc — dropping it returns the new doc to read mode and is the last line of
  // defence against a save reaching the wrong file). Memory hygiene + correctness.
  useEffect(() => {
    const pathById = new Map(tabs.map((tab) => [tab.id, tab.docRef.path]));
    const editor = useEditorStore.getState();
    for (const [id, entry] of Object.entries(editor.entries)) {
      if (pathById.get(id) !== entry.path) editor.remove(id);
    }
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
        run: () => setRelatedOpen((open) => !open),
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
        if (useSettingsStore.getState().focusMode) {
          useSettingsStore.getState().setFocusMode(false);
          event.preventDefault();
          return;
        }
      }
      const inEditable = isEditable(document.activeElement) || paletteOpen || sheetOpen;
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
  }, [registry, paletteOpen, sheetOpen]);

  // --- startup: restore session + settings ------------------------------
  useEffect(() => {
    let active = true;
    void (async () => {
      const { session, settings } = await loadPersisted();
      if (!active) return;
      if (Object.keys(settings).length > 0) useSettingsStore.getState().hydrate(settings);
      if (session) {
        useTabsStore.getState().hydrate(session.tabs, session.activeTabId);
        useSourcesStore.getState().hydrate({ root: session.root, recent: session.recent });
        if (session.root) await openRoot(session.root);
      }
      setHydrated(true);
    })();
    return () => {
      active = false;
    };
  }, [openRoot]);

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
    const onBeforeUnload = () => flushSession();
    const onVisibility = () => {
      if (document.visibilityState === 'hidden') flushSession();
    };
    window.addEventListener('beforeunload', onBeforeUnload);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.removeEventListener('beforeunload', onBeforeUnload);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [hydrated, flushSession]);

  // Tauri's native window close is the ONLY reliable "app is quitting" hook (a
  // webview teardown can cut a best-effort unload write short). We intercept it,
  // enqueue the latest snapshot, DRAIN the writer (await in-flight + coalesced
  // follow-up so it truly hits disk), then close for real. `destroy()` bypasses
  // this handler, so there is no re-entrancy.
  useEffect(() => {
    if (!hydrated || !isTauri()) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void (async () => {
      try {
        const appWindow = getCurrentWindow();
        const off = await appWindow.onCloseRequested(async (event) => {
          event.preventDefault();
          // Unsaved edits? Prompt (Save / Discard / Cancel) instead of quitting —
          // the guard's action performs the actual quit. Otherwise quit now.
          if (useEditorStore.getState().dirtyTabIds().length > 0) {
            setCloseGuard({ kind: 'quit' });
            return;
          }
          try {
            flushSession();
            await sessionWriter.drain();
          } finally {
            await appWindow.destroy();
          }
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
  }, [hydrated, flushSession]);

  // --- persist settings (debounced 1s) ----------------------------------
  useEffect(() => {
    if (!hydrated) return;
    const handle = window.setTimeout(() => {
      const s = useSettingsStore.getState();
      void saveSettings({
        theme: s.theme,
        readingFont: s.readingFont,
        textScaleIndex: s.textScaleIndex,
        sidebarCollapsed: s.sidebarCollapsed,
        outlineVisible: s.outlineVisible,
      });
    }, 1000);
    return () => window.clearTimeout(handle);
  }, [hydrated, theme, readingFont, textScaleIndex, sidebarCollapsed, outlineVisible]);

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
      const entry = tabId ? useEditorStore.getState().entry(tabId) : undefined;
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
      const live = editor.entry(tabId);
      // Only act on an entry that still belongs to this document (the tab may have
      // navigated); otherwise treat it as a plain read-mode reload.
      if (!live || live.path !== path) {
        setReloadNonce((nonce) => nonce + 1);
        toast('Updated');
        return;
      }
      const decision = classifyExternalChange(disk.hash, live.baseHash, live.dirty);
      if (decision === 'self') return; // our own write echoing back — stay silent
      if (decision === 'conflict') {
        // Unsaved edits + a divergent disk: never clobber — raise the banner.
        editor.raiseConflict(tabId, disk.hash, 'external');
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
          onPickFolder={onPickFolder}
          onOpenFile={openFile}
          onOpenRecent={openRef}
          isIgnored={isIgnored}
        />

        <main className="shell__main">
          <TabStrip onRequestClose={requestCloseTab} />
          <div className="shell__reader-row">
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
            />
            {activeTab ? (
              <RelatedRail
                activeDoc={activeDoc}
                adapter={adapter}
                open={relatedOpen}
                onToggle={() => setRelatedOpen((open) => !open)}
                onOpen={openRef}
                workspaceKey={root}
              />
            ) : null}
          </div>
        </main>
      </div>

      <StatusBar
        words={docWords}
        watcher={watcher}
        fileCount={files.length}
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
      {closeGuard ? (
        <UnsavedGuard
          count={
            closeGuard.kind === 'quit' ? useEditorStore.getState().dirtyTabIds().length : 1
          }
          name={
            closeGuard.kind === 'tab'
              ? tabs.find((tab) => tab.id === closeGuard.tabId)?.docRef.name
              : undefined
          }
          onSave={() => void onGuardSave()}
          onDiscard={onGuardDiscard}
          onCancel={() => setCloseGuard(null)}
        />
      ) : null}
      <Toasts />
    </div>
  );
}
