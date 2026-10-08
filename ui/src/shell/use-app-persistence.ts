/** Ordinary renderer persistence and native close lifecycle. Profile-wide
 * preservation joins the actual editor owner with the same accumulated writer. */
import { useCallback, useEffect, useRef, type RefObject } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useShallow } from 'zustand/react/shallow';
import { useTabsStore } from '../stores/tabs.js';
import { useSourcesStore } from '../stores/sources.js';
import { useEditorStore } from '../stores/editor.js';
import { useReaderHistoryStore } from '../features/reader-history/store';
import { createSessionTracker } from '../features/reader-history/session-patches';
import { serializeSession } from '../stores/session.js';
import { useSettingsStore, type SettingsSnapshot } from '../stores/settings.js';
import { isTauri, saveSettings, sessionWriter } from './persistence.js';
import { drainBeforeClose } from './persistence-lifecycle.js';
import { toast } from './toasts.js';
import { installRendererPreservation } from '../features/editor-preservation/bridge';
import { editorAdmission } from '../features/editor-preservation/admission';

function currentSettings(): SettingsSnapshot {
  const s = useSettingsStore.getState();
  return {
    theme: s.theme, readingFont: s.readingFont, textScaleIndex: s.textScaleIndex,
    sidebarCollapsed: s.sidebarCollapsed, outlineVisible: s.outlineVisible,
    relatedRailOpen: s.relatedRailOpen, inspectorOpen: s.inspectorOpen,
    lensSchema: s.lensSchema, reviewer: s.reviewer,
  };
}

export function useAppPersistence({ hydrated, persistenceReady, flushSessionRef, closingRef, requestQuit }: {
  hydrated: boolean;
  persistenceReady: boolean;
  flushSessionRef: RefObject<() => void>;
  closingRef: RefObject<boolean>;
  requestQuit: () => void;
}): void {
  const sessionShape = useTabsStore(useShallow((s) => [s.tabs, s.activeTabId]));
  const sourceShape = useSourcesStore(useShallow((s) => [s.root, s.recent]));
  const readerShape = useReaderHistoryStore((s) => s.data);
  const tracker = useRef<ReturnType<typeof createSessionTracker> | null>(null);
  const settingsShape = useSettingsStore(useShallow((s) => [
    s.theme, s.readingFont, s.textScaleIndex, s.sidebarCollapsed, s.outlineVisible,
    s.relatedRailOpen, s.inspectorOpen, s.lensSchema, s.reviewer,
  ]));
  const failureShown = useRef(false);
  const closing = useRef(false);
  const failed = useCallback(() => {
    if (failureShown.current) return;
    failureShown.current = true;
    toast('Your preferences or session could not be saved. Keep MDReader open and try again.', 8000);
  }, []);
  const flushSession = useCallback(() => {
    if (editorAdmission.denied()) return;
    const t = useTabsStore.getState();
    const s = useSourcesStore.getState();
    const legacy = serializeSession({ root: s.root, activeTabId: t.activeTabId, tabs: t.tabs, recent: s.recent });
    const reader = useReaderHistoryStore.getState();
    // Capture restored legacy state once. Canonical activity never normalizes old arrays.
    tracker.current ??= createSessionTracker(legacy, reader.persisted);
    if (reader.mode === 'transient') {
      sessionWriter.write(legacy);
      return;
    }
    const patch = tracker.current.next(legacy, reader.mode === 'native_atomic' ? reader.data : null);
    if (patch) sessionWriter.write(patch);
  }, []);
  flushSessionRef.current = flushSession;
  const flushAll = useCallback(async () => {
    if (editorAdmission.denied()) throw new Error('Editor preparation is held');
    flushSession();
    await Promise.all([sessionWriter.drain(), saveSettings(currentSettings())]);
    failureShown.current = false;
  }, [flushSession]);

  useEffect(() => {
    if (!persistenceReady) return;
    if (!tracker.current) {
      const t = useTabsStore.getState(), s = useSourcesStore.getState();
      tracker.current = createSessionTracker(serializeSession({ root: s.root, activeTabId: t.activeTabId, tabs: t.tabs, recent: s.recent }), useReaderHistoryStore.getState().persisted);
    }
    const timer = window.setTimeout(() => {
      if (editorAdmission.denied()) return;
      flushSession();
      void sessionWriter.drain().then(() => { failureShown.current = false; }, failed);
    }, 1000);
    return () => window.clearTimeout(timer);
  }, [persistenceReady, flushSession, failed, sessionShape, sourceShape, readerShape]);

  useEffect(() => {
    if (!persistenceReady) return;
    const timer = window.setTimeout(() => {
      if (editorAdmission.denied()) return;
      void saveSettings(currentSettings()).then(() => { failureShown.current = false; }, failed);
    }, 1000);
    return () => window.clearTimeout(timer);
  }, [persistenceReady, settingsShape, failed]);

  useEffect(() => {
    if (!persistenceReady) return;
    const flush = () => { if (!editorAdmission.denied()) void flushAll().catch(failed); };
    const visibility = () => { if (document.visibilityState === 'hidden') flush(); };
    window.addEventListener('beforeunload', flush);
    document.addEventListener('visibilitychange', visibility);
    return () => {
      window.removeEventListener('beforeunload', flush);
      document.removeEventListener('visibilitychange', visibility);
    };
  }, [persistenceReady, flushAll, failed]);

  useEffect(() => {
    if (!persistenceReady || !isTauri()) return;
    let disposed = false;
    let off: (() => void) | undefined;
    void installRendererPreservation({
      latest: () => {
        if (!tracker.current) throw new Error('Session snapshot is not initialized');
        const t = useTabsStore.getState(), s = useSourcesStore.getState();
        const legacy = serializeSession({ root: s.root, activeTabId: t.activeTabId, tabs: t.tabs, recent: s.recent });
        const reader = useReaderHistoryStore.getState();
        const writable = reader.mode === 'native_atomic';
        const patch = tracker.current.peek(legacy, writable ? reader.data : null);
        return { session: sessionWriter.snapshot(patch, writable), settings: currentSettings() };
      },
      paused: () => toast('Preparing reader data. Editing is paused while your drafts and session are captured.', 8000),
      failed: () => toast('Preparation could not be confirmed. Your editor buffers were kept. Editing stays paused until native release.', 10000),
      resumed: () => { void flushAll().catch(failed); },
    }).then((dispose) => {
      if (disposed) dispose();
      else off = dispose;
    }, () => {
      if (!disposed) toast('Profile preservation is unavailable in this window. Your editor buffers were kept.', 8000);
    });
    return () => { disposed = true; off?.(); };
  }, [persistenceReady, flushAll, failed]);

  useEffect(() => {
    if (!hydrated || !isTauri()) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void (async () => {
      try {
        const appWindow = getCurrentWindow();
        const off = await appWindow.onCloseRequested(async (event) => {
          if (closingRef.current) return;
          event.preventDefault();
          if (closing.current) return;
          closing.current = true;
          try {
            if (editorAdmission.denied()) {
              toast('Preparation is still in progress. Keep this window open; your buffers are retained.', 8000);
              return;
            }
            if (persistenceReady) await drainBeforeClose(flushAll());
            if (disposed || editorAdmission.denied()) return;
            if (useEditorStore.getState().dirtyTabIds().length > 0) {
              requestQuit();
              return;
            }
            closingRef.current = true;
            try { await appWindow.destroy(); }
            catch { closingRef.current = false; failed(); }
          } catch { failed(); }
          finally { closing.current = false; }
        });
        if (disposed) off();
        else unlisten = off;
      } catch {
        // Browser previews have no native window-close API.
      }
    })();
    return () => { disposed = true; unlisten?.(); };
  }, [hydrated, persistenceReady, closingRef, flushAll, failed, requestQuit]);
}
