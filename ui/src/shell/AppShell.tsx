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
import { loadPersisted, saveSession, saveSettings } from './persistence.js';
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
  const refreshFiles = useCallback(async (folder: string) => {
    try {
      const list = await listWorkspaceFiles(folder);
      useSourcesStore.getState().setFiles(list);
    } catch {
      /* non-Tauri or unreadable — leave the list as-is */
    }
  }, []);

  // Load a workspace: establish the backend grant (which confines every read to
  // this root), then list files and arm the watcher. Adopts the canonical path
  // the grant returns so file paths and the watched root agree. Never closes
  // tabs — that is the caller's concern (a workspace *switch*), so restore can
  // reuse this without disturbing the session it just hydrated.
  const openRoot = useCallback(async (folder: string) => {
    const sources = useSourcesStore.getState();
    let granted = folder;
    try {
      granted = await setWorkspace(folder);
    } catch {
      /* non-Tauri (website/tests) — no grant to establish; keep the raw path */
    }
    sources.setRoot(granted);
    sources.setLoading(true);
    try {
      const list = await listWorkspaceFiles(granted);
      sources.setFiles(list);
    } catch {
      sources.setFiles([]);
    } finally {
      sources.setLoading(false);
    }
    try {
      await watchWorkspace(granted);
      sources.setWatcher('watching');
    } catch {
      // We committed to the new root but could not arm its watcher: a degraded
      // (but honest) state the StatusBar surfaces rather than a silent "idle".
      sources.setWatcher('error');
    }
  }, []);

  const onPickFolder = useCallback(async () => {
    let picked: string | null;
    try {
      picked = await pickWorkspaceFolder();
    } catch {
      toast('Could not open the folder picker');
      return;
    }
    if (!picked) return;

    // Switching to a different workspace: retire the previous workspace's tabs
    // into Recent so no stale tab survives to refresh against the new root.
    const sources = useSourcesStore.getState();
    if (sources.root && sources.root !== picked) {
      const retired = useTabsStore.getState().closeAll();
      for (const ref of retired) sources.addRecent(ref);
    }
    await openRoot(picked);
  }, [openRoot]);

  // --- open / navigate helpers -----------------------------------------
  const openRef = useCallback((ref: DocRef) => {
    useTabsStore.getState().openTab(ref);
    useSourcesStore.getState().addRecent(ref);
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
    useSourcesStore.getState().addRecent(ref);
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

  // --- persist session (debounced 1s) -----------------------------------
  useEffect(() => {
    if (!hydrated) return;
    const handle = window.setTimeout(() => {
      const t = useTabsStore.getState();
      const s = useSourcesStore.getState();
      void saveSession(
        serializeSession({
          root: s.root,
          activeTabId: t.activeTabId,
          tabs: t.tabs,
          recent: s.recent,
        }),
      );
    }, 1000);
    return () => window.clearTimeout(handle);
  }, [hydrated, tabs, activeTabId, root, recent]);

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

    const reloadActive = (path: string) => {
      if (path === activePathRef.current) {
        setReloadNonce((nonce) => nonce + 1);
        toast('Updated');
      }
    };

    onFsChanged((change) => {
      const existing = reloadTimers.get(change.path);
      if (existing !== undefined) window.clearTimeout(existing);
      if (change.kind === 'continuous') {
        // Still being written — wait for the quiet window before reloading.
        reloadTimers.set(
          change.path,
          window.setTimeout(() => {
            reloadTimers.delete(change.path);
            reloadActive(change.path);
          }, 300),
        );
      } else {
        // Settled: drop any pending trailing reload and apply now.
        reloadTimers.delete(change.path);
        reloadActive(change.path);
      }

      window.clearTimeout(refreshTimer);
      refreshTimer = window.setTimeout(() => {
        const folder = useSourcesStore.getState().root;
        if (folder) void refreshFiles(folder);
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
  }, [refreshFiles]);

  // --- watcher runtime errors → degraded status -------------------------
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    onWatchError(() => {
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
          recent={recent}
          activePath={activeTab?.docRef.path ?? null}
          loading={loading}
          onPickFolder={onPickFolder}
          onOpenFile={openFile}
          onOpenRecent={openRef}
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
          />
        </main>
      </div>

      <StatusBar words={docWords} watcher={watcher} fileCount={files.length} />

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
