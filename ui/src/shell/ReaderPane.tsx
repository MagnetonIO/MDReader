/**
 * The reading surface for the active tab — and only the active tab is ever
 * mounted here, honouring the "one Reader" invariant. It reads the document
 * through the thin fs command layer, parses it with the real
 * `@mdreader/markdown` pipeline (off-thread for large docs via `parseMarkdownAuto`),
 * and renders the shared `MarkdownReader`. It also:
 *   - persists a scroll anchor (the active heading's source line) into the tab,
 *     restoring it on load / reopen / live-refresh;
 *   - drives the OutlineRail from scroll-spy and scrolls on outline selection;
 *   - resolves wikilinks/relative links against the workspace file list;
 *   - restyles per the reading-font / text-size settings without touching
 *     `@mdreader/ui` (it overrides the reader's CSS custom properties on a wrapper).
 *
 * Light editing (P4) layers on top without disturbing the read path: a per-tab
 * view mode (read / source / split) drawn from the editor store swaps the reading
 * pane for a CodeMirror `SourcePane`, or shows both side-by-side in split with an
 * approximate line-based scroll sync. The `ConflictBanner` renders above the body
 * whenever the open document has a pending conflict.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { MarkdownReader, PlatformProvider } from '@mdreader/ui';
import type { MarkdownReaderHandle } from '@mdreader/ui';
import { parseMarkdownAuto } from '@mdreader/markdown/worker';
import type { OutlineItem, ParsedDoc } from '@mdreader/markdown';

import { readDoc, resolveWikilink } from '../platform/fs.js';
import type { WorkspaceFile } from '../platform/fs.js';
import { createReaderPlatform } from '../platform/reader-platform.js';
import { findByWikiTarget, headingLineFromOutline, navigateWikiLink } from './wikilink.js';
import { useWikiDimming } from './wiki-dimming.js';
import { useDocLoader } from './doc-loader.js';
import { OutlineRail } from './OutlineRail.js';
import { ReaderToolbar } from './ReaderToolbar.js';
import { SourcePane } from './SourcePane.js';
import { ConflictBanner } from './ConflictBanner.js';
import { useEditorStore } from '../stores/editor.js';
import type { EditorMode } from '../stores/editor.js';
import type { ReadingFont } from '../stores/settings.js';
import { readingAppearance } from '../features/reader/appearance';
import { isLocalDocument, type DocRef, type ScrollPos, type TabState } from '../stores/types.js';

/**
 * A pending provenance jump, carrying the WHOLE cited range.
 *
 * `startLine` is where the reader lands; `endLine` is how far the citation
 * actually reaches. The two are separate on purpose: a violation that names
 * lines 41–47 is a claim about seven lines, and a jump that kept only the first
 * one would silently shrink it to a claim about one. Callers that genuinely
 * cite a single line pass the same value twice — deliberately, at the call site,
 * where the reason can be read.
 *
 * `nonce` makes each jump distinct so re-jumping to the same range re-fires.
 */
export interface JumpTarget {
  path: string;
  startLine: number;
  endLine: number;
  nonce: number;
}

export interface ReaderPaneProps {
  tab: TabState | null;
  root: string | null;
  files: WorkspaceFile[];
  outlineVisible: boolean;
  readingFont: ReadingFont;
  textScale: number;
  reloadNonce: number;
  onToggleOutline: () => void;
  onNavigate: (ref: DocRef) => void;
  onPersistScroll: (scroll: ScrollPos) => void;
  onDocMeta: (meta: { words: number } | null) => void;
  onPickFolder: () => void;
  onMissingWikiLink: (target: string) => void;
  /** A wikilink matched several notes — surface the candidate set. */
  onAmbiguousWikiLink?: (target: string, candidates: DocRef[]) => void;
  /**
   * A pending provenance jump: land on the range's first line in the document at
   * `path`, and mark every block the range covers. The reader consumes it once
   * the target renders.
   */
  jump?: JumpTarget | null;
  /** Switch the active tab's view mode (seeds the editor buffer on first entry). */
  onSetMode: (mode: EditorMode) => void;
  /** Guarded save of the active document (Cmd+S / editor Cmd+S). */
  onSaveActive: () => void;
  /** Conflict banner: discard local edits and adopt the on-disk version. */
  onReloadActive: () => void;
  /** Conflict banner: force-overwrite the on-disk version with the buffer. */
  onOverwriteActive: () => void;
}

function dirname(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '');
  const index = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  return index >= 0 ? normalized.slice(0, index) : normalized;
}

function countWords(content: string): number {
  const trimmed = content.trim();
  return trimmed.length === 0 ? 0 : trimmed.split(/\s+/).length;
}

function resolveRelative(files: WorkspaceFile[], docDir: string, href: string): WorkspaceFile | null {
  const clean = href.split('#')[0]!.split('?')[0]!;
  const base = docDir.replace(/[\\/]+$/, '');
  const stack = base.split(/[\\/]+/);
  for (const part of clean.split(/[\\/]+/)) {
    if (part === '' || part === '.') continue;
    if (part === '..') stack.pop();
    else stack.push(part);
  }
  const resolved = stack.join('/');
  return files.find((file) => file.path.replace(/\\/g, '/') === resolved) ?? null;
}

export function ReaderPane({
  tab,
  root,
  files,
  outlineVisible,
  readingFont,
  textScale,
  reloadNonce,
  onToggleOutline,
  onNavigate,
  onPersistScroll,
  onDocMeta,
  onPickFolder,
  onMissingWikiLink,
  onAmbiguousWikiLink,
  jump,
  onSetMode,
  onSaveActive,
  onReloadActive,
  onOverwriteActive,
}: ReaderPaneProps) {
  const readerRef = useRef<MarkdownReaderHandle | null>(null);
  // A separate reader for the split-mode preview, so its scroll-sync never
  // entangles with the full-featured read-mode reader's scroll/jump effects.
  const splitPreviewRef = useRef<MarkdownReaderHandle | null>(null);
  const [activeSlug, setActiveSlug] = useState<string | undefined>(undefined);
  // A brief highlight range for the provenance-jump pulse (cleared on a timer).
  const [pulse, setPulse] = useState<{ start: number; end: number } | null>(null);
  const pulseTimer = useRef<number | undefined>(undefined);
  const consumedJump = useRef(0);

  const localRef = tab && isLocalDocument(tab.docRef) ? tab.docRef : null;
  const path = localRef?.path ?? null;
  const tabId = tab?.id ?? null;
  // The active (tab, document)'s editing state (undefined ⇒ plain read mode).
  // Keyed by BOTH tab and path, so a tab that has navigated to another document
  // shows THAT document's state (read mode until edited), while the buffer of the
  // doc it navigated away from stays intact under its own key.
  const editEntry = useEditorStore((store) =>
    tabId && path !== null ? store.entry(tabId, path) : undefined,
  );
  const mode: EditorMode = editEntry?.mode ?? 'read';
  const buffer = editEntry?.buffer;

  // Live preview for split mode: re-parse the buffer (debounced) so the rendered
  // side tracks what is being typed, independent of what is on disk.
  const [previewDoc, setPreviewDoc] = useState<ParsedDoc | undefined>(undefined);
  useEffect(() => {
    if (mode !== 'split' || buffer === undefined) return;
    let cancelled = false;
    const handle = window.setTimeout(() => {
      void parseMarkdownAuto(buffer).then((doc) => {
        if (!cancelled) setPreviewDoc(doc);
      });
    }, 150);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [mode, buffer]);

  // Live mirrors of the active document + workspace, so an in-flight wikilink
  // resolution can tell at navigation time whether its click context still holds
  // (the user may have switched docs or workspaces while it resolved).
  const pathRef = useRef(path);
  pathRef.current = path;
  const rootRef = useRef(root);
  rootRef.current = root;
  // A heading a wikilink is aiming at, keyed by the TARGET doc path: recorded at
  // resolve time and consumed once that doc finishes loading (cross-doc heading
  // jumps can't scroll until their document has rendered).
  const pendingLineByPath = useRef<Map<string, number>>(new Map());
  // Like the above but for the CLIENT-SIDE fallback path, which has only the
  // heading TEXT (the backend missed, so it returned no source line): resolved
  // against the target doc's outline once it renders.
  const pendingHeadingByPath = useRef<Map<string, string>>(new Map());
  // The scroll position to restore for the freshly-loaded doc — captured in a
  // ref so scroll writes (which happen after restore) never retrigger a restore.
  const scrollToRestore = useRef<ScrollPos>({ block: 0, offset: 0 });
  scrollToRestore.current = tab?.scroll ?? { block: 0, offset: 0 };

  // Load + parse the active document. Workspace-aware: `root` is a dependency, so
  // a switch (root change) with the same tab mounted reruns cleanup — an in-flight
  // parse for the OLD workspace is cancelled rather than landing under the new one.
  const readContent = useCallback(async (docPath: string) => (await readDoc(docPath)).content, []);
  const state = useDocLoader({
    path,
    root,
    reloadNonce,
    read: readContent,
    parse: parseMarkdownAuto,
    dirOf: dirname,
    wordsOf: countWords,
    onMeta: onDocMeta,
  });

  // Restore the block+offset scroll position once, after a fresh document
  // renders. Works for headingless docs and mid-block offsets alike (the
  // anchor is a block index + pixel offset, not a heading line).
  useEffect(() => {
    if (state.status !== 'ready') return;
    const { block, offset } = scrollToRestore.current;
    if (block <= 0 && offset <= 0) return;
    const frame = requestAnimationFrame(() => readerRef.current?.scrollToBlock(block, offset));
    return () => cancelAnimationFrame(frame);
    // Intentionally keyed on the loaded doc only, not the tab (scroll writes
    // must not retrigger a restore).
  }, [state.doc]);

  // Consume a pending provenance jump: once the jump's OWN document is on
  // screen, land on the range's first line and mark EVERY block the range
  // covers — the reader's `highlightRange` is matched against each block's
  // stamped `[data-line-start, data-line-end]`, so the marked extent is the
  // cited extent and not merely its opening line. Declared AFTER the
  // scroll-restore effect so its rAF runs later and wins (an explicit jump beats
  // a restore).
  useEffect(() => {
    if (state.status !== 'ready' || !path || !jump) return;
    // `ready` is not enough. A jump to ANOTHER document arrives while the
    // previous one is still mounted, so the loaded path is checked too —
    // otherwise the cited lines get marked in the file you were reading before.
    if (state.path !== path || jump.path !== path) return;
    if (jump.nonce === consumedJump.current) return;
    const { startLine, endLine } = jump;
    const frame = requestAnimationFrame(() => {
      // Marked as consumed only once it has actually been APPLIED. Consuming at
      // schedule time silently swallowed every cross-document jump: the effect
      // re-ran when the target finished loading, cancelled the pending frame,
      // and then declined to reschedule because the jump "had been" handled.
      consumedJump.current = jump.nonce;
      readerRef.current?.scrollToLine(startLine);
      setPulse({ start: startLine, end: endLine });
      window.clearTimeout(pulseTimer.current);
      pulseTimer.current = window.setTimeout(() => setPulse(null), 1400);
    });
    return () => cancelAnimationFrame(frame);
  }, [state.doc, state.path, path, jump]);
  useEffect(() => () => window.clearTimeout(pulseTimer.current), []);

  // Once a wikilink-target document has rendered, jump to the heading line the
  // link asked for (recorded at resolve time, keyed by the target path).
  useEffect(() => {
    if (state.status !== 'ready' || !path) return;
    const line = pendingLineByPath.current.get(path);
    if (line === undefined) return;
    pendingLineByPath.current.delete(path);
    const frame = requestAnimationFrame(() => readerRef.current?.scrollToLine(line));
    return () => cancelAnimationFrame(frame);
  }, [state.doc, path]);

  // Same, for the CLIENT-SIDE fallback: we only have the heading TEXT, so resolve
  // it to a source line against the freshly-parsed outline before scrolling.
  useEffect(() => {
    if (state.status !== 'ready' || !path) return;
    const heading = pendingHeadingByPath.current.get(path);
    if (heading === undefined) return;
    pendingHeadingByPath.current.delete(path);
    const line = headingLineFromOutline(state.doc?.outline ?? [], heading);
    if (line === null) return;
    const frame = requestAnimationFrame(() => readerRef.current?.scrollToLine(line));
    return () => cancelAnimationFrame(frame);
  }, [state.doc, path]);

  // Persist the scroll anchor, debounced, so store churn (and its re-renders)
  // stays off the scroll's hot path while still capturing the resting position.
  const persistTimer = useRef<number | undefined>(undefined);
  const persistScroll = useCallback(
    (anchor: ScrollPos) => {
      window.clearTimeout(persistTimer.current);
      persistTimer.current = window.setTimeout(() => onPersistScroll(anchor), 200);
    },
    [onPersistScroll],
  );
  useEffect(() => () => window.clearTimeout(persistTimer.current), []);

  const outline = state.doc?.outline ?? [];

  const handleActiveHeading = useCallback((slug: string | undefined) => {
    setActiveSlug(slug);
  }, []);

  const onOutlineSelect = useCallback((item: OutlineItem) => {
    readerRef.current?.scrollToLine(item.lineStart);
  }, []);

  // The reader's synchronous "does this target resolve?" predicate — reflects
  // ACTUAL index resolution (filename OR title), cached in a bounded LRU that
  // re-renders on new resolutions and re-resolves evicted keys. See
  // `useWikiDimming`.
  const resolveWiki = useCallback(
    (target: string) =>
      resolveWikilink(path ?? '', target, null).then(
        (res) => res.status === 'resolved' || res.status === 'ambiguous',
      ),
    [path],
  );
  const wikiFallback = useCallback(
    (target: string) => findByWikiTarget(files, target) !== null,
    [files],
  );
  const resolveWikiLink = useWikiDimming({
    path,
    files,
    resolve: resolveWiki,
    fallback: wikiFallback,
  });

  // Resolve a clicked wikilink through the index (filename then title match):
  // one match opens a tab (and scrolls to the heading line when one was given),
  // several surface as candidates, none falls back to the local file list before
  // finally toasting "no note named …".
  const handleWikiLink = useCallback(
    (target: string, heading: string | null) => {
      const fromPath = path;
      const fromRoot = root;
      void navigateWikiLink({
        resolve: () => resolveWikilink(fromPath ?? '', target, heading),
        // Discard the navigation if the click's document or workspace changed
        // while the index resolved (thread the epoch/document ref → re-check).
        stillValid: () => pathRef.current === fromPath && rootRef.current === fromRoot,
        onResolved: (ref, line) => {
          if (line !== null) {
            if (ref.path === fromPath) readerRef.current?.scrollToLine(line);
            else pendingLineByPath.current.set(ref.path, line);
          }
          onNavigate(ref);
        },
        onAmbiguous: (candidates) => {
          if (onAmbiguousWikiLink) onAmbiguousWikiLink(target, candidates);
          else onMissingWikiLink(target);
        },
        onUnresolved: () => {
          // Authoritative miss → exact-basename fallback, honoring the requested
          // heading (scrolled once the target renders, via its outline).
          const match = findByWikiTarget(files, target);
          if (match) {
            if (heading) pendingHeadingByPath.current.set(match.path, heading);
            onNavigate({ path: match.path, name: match.name });
          } else onMissingWikiLink(target);
        },
      });
    },
    [path, root, files, onNavigate, onMissingWikiLink, onAmbiguousWikiLink],
  );

  const onReaderNavigate = useCallback(
    (event: { kind: string; target?: string; heading?: string | null; slug?: string; href?: string }) => {
      if (event.kind === 'wikilink' && event.target) {
        handleWikiLink(event.target, event.heading ?? null);
      } else if (event.kind === 'anchor' && event.slug) {
        const item = outline.find((entry) => entry.slug === event.slug);
        if (item) readerRef.current?.scrollToLine(item.lineStart);
      } else if (event.kind === 'relative' && event.href && state.dir) {
        const match = resolveRelative(files, state.dir, event.href);
        if (match) onNavigate({ path: match.path, name: match.name });
      }
    },
    [files, outline, onNavigate, state.dir, handleWikiLink],
  );

  const platform = useMemo(() => createReaderPlatform(state.dir ?? null), [state.dir]);

  const hostStyle = useMemo(() => readingAppearance(readingFont, textScale), [textScale, readingFont]);

  // Editor callbacks (stable): a keystroke updates the store's buffer; the split
  // editor scroll drives the preview via an approximate line map.
  const onEditBuffer = useCallback(
    (value: string) => {
      if (tabId && path !== null) useEditorStore.getState().edit(tabId, path, value);
    },
    [tabId, path],
  );
  const syncPreviewToLine = useCallback((line: number) => {
    splitPreviewRef.current?.scrollToLine(line);
  }, []);

  if (!tab) {
    return (
      <div className="reader-region">
        <div className="reader-empty">
          <h2 className="reader-empty__title">A calm place to read your markdown</h2>
          <p className="reader-empty__hint">
            Open a workspace folder to browse its notes, then press a file — or use{' '}
            <span className="kbd">Cmd P</span> to jump straight to one.
          </p>
          {root ? null : (
            <button type="button" className="btn-primary" onClick={onPickFolder}>
              Open folder…
            </button>
          )}
        </div>
      </div>
    );
  }

  const conflict = editEntry?.conflict ?? null;
  const showEditor = mode !== 'read' && buffer !== undefined;
  const showReader = mode !== 'source';
  const splitDoc = mode === 'split' ? (previewDoc ?? state.doc) : state.doc;

  return (
    <div className="reader-region">
      <ReaderToolbar
        docRef={localRef}
        root={root}
        outlineVisible={outlineVisible}
        onToggleOutline={onToggleOutline}
        hasOutline={mode !== 'source' && outline.length > 0}
        mode={mode}
        onSetMode={onSetMode}
        canEdit
      />
      {conflict ? (
        <ConflictBanner
          source={conflict.source}
          phase={conflict.phase}
          onReload={onReloadActive}
          onKeepMine={() => {
            if (tabId && path !== null) useEditorStore.getState().toConfirmOverwrite(tabId, path);
          }}
          onOverwrite={onOverwriteActive}
          onCancel={() => {
            if (tabId && path !== null) useEditorStore.getState().clearConflict(tabId, path);
          }}
        />
      ) : null}
      <div className={`reader-body reader-body--${mode}`}>
        {showEditor ? (
          <div className="source-region">
            <SourcePane
              value={buffer!}
              onChange={onEditBuffer}
              onSave={onSaveActive}
              onToggleMode={() => onSetMode('read')}
              onScrollLine={mode === 'split' ? syncPreviewToLine : undefined}
            />
          </div>
        ) : null}
        {showReader ? (
          <div className="reader-host" style={hostStyle}>
            {mode === 'split' ? (
              splitDoc ? (
                <PlatformProvider platform={platform}>
                  <MarkdownReader
                    ref={splitPreviewRef}
                    doc={splitDoc}
                    onNavigate={onReaderNavigate}
                    resolveWikiLink={resolveWikiLink}
                  />
                </PlatformProvider>
              ) : null
            ) : state.status === 'error' ? (
              <div className="reader-empty">
                <p className="reader-empty__hint">Could not open this document: {state.error}</p>
              </div>
            ) : state.doc ? (
              <PlatformProvider platform={platform}>
                <MarkdownReader
                  ref={readerRef}
                  doc={state.doc}
                  activeSlug={activeSlug}
                  highlightRange={pulse ?? undefined}
                  onActiveHeadingChange={handleActiveHeading}
                  onNavigate={onReaderNavigate}
                  resolveWikiLink={resolveWikiLink}
                  onScrollAnchorChange={persistScroll}
                />
              </PlatformProvider>
            ) : null}
          </div>
        ) : null}
        {mode === 'read' && outlineVisible && state.doc ? (
          <OutlineRail items={outline} activeSlug={activeSlug} onSelect={onOutlineSelect} />
        ) : null}
      </div>
    </div>
  );
}
