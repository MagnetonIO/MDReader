/**
 * The MDReader application shell: overlay titlebar, collapsible sidebar, tab
 * strip, reading pane, status bar, and the keyboard-first overlays (file-jump
 * palette + shortcut sheet). It owns the wiring the leaf components stay ignorant
 * of: one KeymapRegistry driving both dispatch and the sheet, session/settings
 * persistence, the workspace watcher's live-reload, and theme application.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import {
  listWorkspaceFiles,
  onFsChanged,
  onWatchError,
  pickWorkspaceFolder,
  setWorkspace,
  startImport,
  watchWorkspace,
  type WorkspaceFile,
} from '../platform/fs.js';
import { KeymapRegistry, detectMac } from '../keymap/registry.js';
import type { Binding } from '../keymap/registry.js';
import { selectActiveTab, useTabsStore } from '../stores/tabs.js';
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
import { reindexOnChange } from './reindex.js';
import { LoadingGate } from './loading-gate.js';
import { Titlebar } from './Titlebar.js';
import { Sidebar } from './Sidebar.js';
import { TabStrip } from './TabStrip.js';
import { ReaderPane } from './ReaderPane.js';
import { StatusBar } from './StatusBar.js';
import { FileJumpPalette } from './FileJumpPalette.js';
import { ShortcutSheet } from './ShortcutSheet.js';
import type { ShortcutGroup } from './ShortcutSheet.js';
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
  const [sheetOpen, setSheetOpen] = useState(false);
  const [reloadNonce, setReloadNonce] = useState(0);
  const [docWords, setDocWords] = useState<number | null>(null);
  const [hydrated, setHydrated] = useState(false);

  const activePathRef = useRef<string | null>(null);
  activePathRef.current = activeTab?.docRef.path ?? null;

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

  // Bring the index in step with disk, then re-list. A fresh open or a settled
  // fs change runs an incremental import THEN refreshes the listing; while an
  // import is already in flight (backend-coalesced) it refreshes alone. `epoch`
  // scopes the continuations so a switch mid-flight discards them.
  const triggerReindex = useCallback(
    (folder: string, epoch: number) => {
      if (!isTauri()) return Promise.resolve('refreshed' as const);
      const phase = useSourcesStore.getState().importState?.phase;
      return reindexOnChange({
        importing: phase === 'importing' || phase === 'loading',
        startImport: () => runImport(folder, epoch),
        refreshFiles: () => refreshFiles(folder, epoch),
      });
    },
    [runImport, refreshFiles],
  );

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
      // This open supersedes any prior in-flight open: bump the request
      // generation and thread it through every continuation below.
      const epoch = (openEpochRef.current += 1);

      // Website / tests without a Tauri backend: no grant/watch IPC. Open the
      // folder best-effort so the shell still renders content.
      if (!isTauri()) {
        sources.setRoot(folder);
        sources.setWatcher('idle');
        await listInto(folder, epoch);
        return true;
      }

      const previousRoot = sources.root;
      const isSwitch = previousRoot !== null && previousRoot !== folder;

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
          // failure transiently instead. As the NEWEST open, its failure must
          // clear any loading an older open left spinning.
          loadingGateRef.current.fail(loadingSink);
          toast('Could not switch workspace — staying on the current folder');
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
        void triggerReindex(g, epoch);
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
      void triggerReindex(grant.root, epoch);
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
    async (folder: string): Promise<boolean> => {
      const sources = useSourcesStore.getState();
      const previousRoot = sources.root;
      if (previousRoot === folder) return true;

      const committed = await openRoot(folder);
      if (committed && previousRoot) {
        const retired = useTabsStore.getState().closeAll();
        for (const ref of retired) sources.addRecent(ref, previousRoot);
      }
      return committed;
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
    const folder = useSourcesStore.getState().root;
    if (folder) void triggerReindex(folder, openEpochRef.current);
  }, [triggerReindex]);

  // --- open / navigate helpers -----------------------------------------
  const openRef = useCallback((ref: DocRef) => {
    const root = useSourcesStore.getState().root;
    useTabsStore.getState().openTab(ref);
    useSourcesStore.getState().addRecent(ref, root);
  }, []);

  const openFile = useCallback(
    (file: WorkspaceFile) => {
      openRef({ path: file.path, name: file.name });
      setPaletteOpen(false);
    },
    [openRef],
  );

  const onNavigate = useCallback((ref: DocRef) => {
    const active = useTabsStore.getState().activeTabId;
    if (!active) {
      useTabsStore.getState().openTab(ref);
    } else {
      useTabsStore.getState().navigateInTab(active, ref);
    }
    useSourcesStore.getState().addRecent(ref, useSourcesStore.getState().root);
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
      useTabsStore.getState().openTab(candidates[0]);
      useSourcesStore.getState().addRecent(candidates[0], useSourcesStore.getState().root);
    }
  }, []);

  const focusOutline = useCallback(() => {
    useSettingsStore.setState({ outlineVisible: true });
    requestAnimationFrame(() => {
      document.querySelector<HTMLElement>('.outline-rail a')?.focus();
    });
  }, []);

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
        run: () => setPaletteOpen(true),
      },
      {
        id: 'search',
        keys: 'Mod+K',
        description: 'Search (arrives in P4)',
        group: 'Search',
        run: () => toast('Search arrives in P4'),
      },
      {
        id: 'new-tab',
        keys: 'Mod+T',
        description: 'New tab',
        group: 'Tabs',
        run: () => setPaletteOpen(true),
      },
      {
        id: 'close-tab',
        keys: 'Mod+W',
        description: 'Close tab',
        group: 'Tabs',
        run: () => {
          const id = tabsApi().activeTabId;
          if (id) tabsApi().closeTab(id);
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
  }, [isMac, focusOutline]);

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

    const reloadActive = (path: string, arrivalGen: number) => {
      // A switch between event arrival and this (debounced) fire discards it.
      if (openEpochRef.current !== arrivalGen) return;
      if (path === activePathRef.current) {
        setReloadNonce((nonce) => nonce + 1);
        toast('Updated');
      }
    };

    onFsChanged((change) => {
      // Discard events from a PREVIOUS watcher (still draining after a switch) —
      // they belong to a workspace we have moved off.
      if (change.generation !== watcherGenRef.current) return;
      // Capture the open generation at event ARRIVAL; the debounce timers below
      // fire later and re-check it, so a switch mid-debounce discards them.
      const arrivalGen = openEpochRef.current;
      const existing = reloadTimers.get(change.path);
      if (existing !== undefined) window.clearTimeout(existing);
      if (change.kind === 'continuous') {
        // Still being written — wait for the quiet window before reloading.
        reloadTimers.set(
          change.path,
          window.setTimeout(() => {
            reloadTimers.delete(change.path);
            reloadActive(change.path, arrivalGen);
          }, 300),
        );
      } else {
        // Settled: drop any pending trailing reload and apply now.
        reloadTimers.delete(change.path);
        reloadActive(change.path, arrivalGen);
      }

      window.clearTimeout(refreshTimer);
      refreshTimer = window.setTimeout(() => {
        // Re-check the captured generation at fire: a switch during the debounce
        // window means this event belongs to a workspace we have left → discard.
        if (openEpochRef.current !== arrivalGen) return;
        const folder = useSourcesStore.getState().root;
        // A settled change drives an incremental import THEN a re-list (or just a
        // re-list when an import is already running) so the index — and thus
        // wikilink/title resolution — stays in step with disk.
        if (folder) void triggerReindex(folder, arrivalGen);
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
          <TabStrip />
          <ReaderPane
            tab={activeTab ?? null}
            root={root}
            files={files}
            outlineVisible={outlineVisible}
            readingFont={readingFont}
            textScale={textScale}
            reloadNonce={reloadNonce}
            onToggleOutline={() => useSettingsStore.getState().toggleOutline()}
            onNavigate={onNavigate}
            onPersistScroll={onPersistScroll}
            onDocMeta={onDocMeta}
            onPickFolder={onPickFolder}
            onMissingWikiLink={onMissingWikiLink}
            onAmbiguousWikiLink={onAmbiguousWikiLink}
          />
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
        <FileJumpPalette
          files={files}
          root={root}
          onOpen={openFile}
          onClose={() => setPaletteOpen(false)}
        />
      ) : null}
      {sheetOpen ? (
        <ShortcutSheet groups={shortcutGroups} onClose={() => setSheetOpen(false)} />
      ) : null}
      <Toasts />
    </div>
  );
}
