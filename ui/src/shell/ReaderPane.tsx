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
import type { OutlineItem } from '@mdreader/markdown';

import { readDoc, resolveWikilink } from '../platform/fs.js';
import type { WorkspaceFile } from '../platform/fs.js';
import { createReaderPlatform } from '../platform/reader-platform.js';
import { findWikiTargets, headingLineFromOutline, navigateWikiLink } from './wikilink.js';
import { useWikiDimming } from './wiki-dimming.js';
import { useDocLoader } from './doc-loader.js';
import { OutlineRail } from './OutlineRail.js';
import { ReaderToolbar } from './ReaderToolbar.js';
import { SourcePane, type SourcePaneCommands } from './SourcePane.js';
import { ConflictBanner } from './ConflictBanner.js';
import { useEditorStore } from '../stores/editor.js';
import type { EditorMode } from '../stores/editor.js';
import type { ReadingFont } from '../stores/settings.js';
import { readingAppearance } from '../features/reader/appearance';
import { isLocalDocument, type DocRef, type ScrollPos, type TabState } from '../stores/types.js';
import { resolveWorkspaceLink } from './relative-link';
import { LocalImageProvider, LocalSourceImage } from '../features/reader/LocalSourceImage';
import { AttachmentLayer } from '../features/reader/AttachmentLayer';
import { isAttachmentLink } from '../features/reader/attachment-preview';
import { useWorkspaceAssets } from './workspace-assets';
import { WorkspaceLinkChoices } from './WorkspaceLinkChoices';

import { EditorToolbar } from '../features/editor-controls/EditorToolbar';
import { useEditorView } from '../features/editor-controls/useEditorView';
import { useDraftPreview } from '../features/editor-controls/useDraftPreview';
import { useDraftCopy } from '../features/editor-controls/useDraftCopy';
import { useEditorExit } from '../features/editor-controls/useEditorExit';
import { EditorExitNotice } from '../features/editor-controls/EditorExitNotice';

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
  onSaveActive: () => void | Promise<unknown>;
  /** Conflict banner: discard local edits and adopt the on-disk version. */
  onReloadActive: () => void;
  /** Conflict banner: force-overwrite the on-disk version with the buffer. */
  onOverwriteActive: () => void;
  /** The active Library source is read-only; editing is not offered. */
  readOnly?: boolean;
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
  jump,
  onSetMode,
  onSaveActive,
  onReloadActive,
  onOverwriteActive,
  readOnly = false,
}: ReaderPaneProps) {
  const [linkNotice, setLinkNotice] = useState<string | null>(null);
  const [linkChoices, setLinkChoices] = useState<{ path: string | null; root: string | null; action: number; candidates: DocRef[]; heading: string | null } | null>(null);
  const linkAction = useRef(0);
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
  useEffect(() => { setLinkNotice(null); setLinkChoices(null); }, [path, root]);
  const tabId = tab?.id ?? null;
  // The active (tab, document)'s editing state (undefined ⇒ plain read mode).
  // Keyed by BOTH tab and path, so a tab that has navigated to another document
  // shows THAT document's state (read mode until edited), while the buffer of the
  // doc it navigated away from stays intact under its own key.
  const editEntry = useEditorStore((store) =>
    tabId && path !== null ? store.entry(tabId, path) : undefined,
  );
  const { view: mode, narrow } = useEditorView(editEntry?.mode ?? 'read');
  const buffer = editEntry?.buffer;
  const previewIdentity = useMemo(() => ({}), [tabId, path, root]);
  const { doc: previewDoc, error: previewError } = useDraftPreview(previewIdentity, buffer, mode === 'split' || mode === 'preview');
  const commandsRef = useRef<SourcePaneCommands | null>(null);
  const copy = useDraftCopy(`${tabId}:${root}:${path}`);
  const exit = useEditorExit(tabId, path, () => onSetMode('read'));
  const [saving, setSaving] = useState<object | null>(null);
  const saveFlight = useRef(false);
  const localSaving = saving === previewIdentity;
  const saveActive = async () => {
    if (saveFlight.current) return;
    saveFlight.current = true;
    const identity = previewIdentity;
    setSaving(identity);
    try { await onSaveActive(); }
    finally { saveFlight.current = false; setSaving(value => value === identity ? null : value); }
  };

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
  const readContent = useCallback(async (docPath: string) => {
    const value = await readDoc(docPath);
    return { content: value.content, receipt: value.originReceipt };
  }, []);
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
  const currentOrigin = state.status === 'ready' && state.path === path && state.root === root ? state.receipt : undefined;
  const assets = useWorkspaceAssets(currentOrigin, linkAction, setLinkNotice);

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
    if (state.status !== 'ready' || !path || state.path !== path) return;
    const line = pendingLineByPath.current.get(path);
    if (line === undefined) return;
    const frame = requestAnimationFrame(() => {
      pendingLineByPath.current.delete(path);
      (mode === 'split' || mode === 'preview' ? splitPreviewRef.current : readerRef.current)?.scrollToLine(line);
    });
    return () => cancelAnimationFrame(frame);
  }, [state.doc, state.path, path, mode]);

  // Same, for the CLIENT-SIDE fallback: we only have the heading TEXT, so resolve
  // it to a source line against the freshly-parsed outline before scrolling.
  useEffect(() => {
    if (state.status !== 'ready' || !path || state.path !== path) return;
    const heading = pendingHeadingByPath.current.get(path);
    if (heading === undefined) return;
    const frame = requestAnimationFrame(() => {
      pendingHeadingByPath.current.delete(path);
      const handle = mode === 'split' || mode === 'preview' ? splitPreviewRef.current : readerRef.current;
      if (handle?.scrollToFragment(heading)) return;
      const line = headingLineFromOutline(state.doc?.outline ?? [], heading);
      if (line !== null) handle?.scrollToLine(line);
      else setLinkNotice('The requested location is unavailable in this document.');
    });
    return () => cancelAnimationFrame(frame);
  }, [state.doc, state.path, path, mode]);

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
    linkAction.current++;
    (mode === 'split' || mode === 'preview' ? splitPreviewRef.current : readerRef.current)?.scrollToLine(item.lineStart);
  }, [mode]);

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
    (target: string) => findWikiTargets(files, target).length > 0,
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
      const selectedAction = linkAction.current;
      let fragment: string | null;
      let fallbackTarget: string;
      try { fragment = heading ? decodeURIComponent(heading) : null; fallbackTarget = decodeURIComponent(target); }
      catch { setLinkNotice('This Markdown link is invalid.'); return; }
      if (fragment && [...fragment].some(char => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127)) { setLinkNotice('This Markdown link is invalid.'); return; }
      void navigateWikiLink({
        // Candidate selection is independent of the renderer's final IDs.
        resolve: () => resolveWikilink(fromPath ?? '', target, null),
        // Discard the navigation if the click's document or workspace changed
        // while the index resolved (thread the epoch/document ref → re-check).
        stillValid: () => pathRef.current === fromPath && rootRef.current === fromRoot && linkAction.current === selectedAction,
        onResolved: (ref, line) => {
          if (fragment) {
            if (ref.path === fromPath) {
              const handle = mode === 'split' || mode === 'preview' ? splitPreviewRef.current : readerRef.current;
              if (!handle?.scrollToFragment(fragment)) {
                const headingLine = headingLineFromOutline(outline, fragment);
                if (headingLine !== null) handle?.scrollToLine(headingLine);
                else setLinkNotice('The requested location is unavailable in this document.');
              }
              return;
            }
            pendingHeadingByPath.current.set(ref.path, fragment);
          }
          else if (line !== null) {
            if (ref.path === fromPath) readerRef.current?.scrollToLine(line);
            else pendingLineByPath.current.set(ref.path, line);
          }
          onNavigate(ref);
        },
        onAmbiguous: (candidates) => {
          setLinkChoices({ path: fromPath, root: fromRoot, action: selectedAction, candidates, heading: fragment });
        },
        onUnresolved: () => {
          // Authoritative miss → exact-basename fallback, honoring the requested
          // heading (scrolled once the target renders, via its outline).
          const matches = findWikiTargets(files, fallbackTarget);
          if (matches.length > 1) {
            setLinkChoices({ path: fromPath, root: fromRoot, action: selectedAction,
              candidates: matches.map(({ path, name }) => ({ path, name })), heading: fragment });
            return;
          }
          const match = matches[0];
          if (match) {
            if (fragment && match.path === fromPath) {
              const handle = mode === 'split' || mode === 'preview' ? splitPreviewRef.current : readerRef.current;
              if (!handle?.scrollToFragment(fragment)) {
                const line = headingLineFromOutline(outline, fragment);
                if (line !== null) handle?.scrollToLine(line);
                else setLinkNotice('The requested location is unavailable in this document.');
              }
              return;
            }
            if (fragment) pendingHeadingByPath.current.set(match.path, fragment);
            onNavigate({ path: match.path, name: match.name });
          } else onMissingWikiLink(target);
        },
      });
    },
    [path, root, files, mode, outline, onNavigate, onMissingWikiLink],
  );

  const onReaderNavigate = useCallback(
    (event: { kind: string; target?: string; heading?: string | null; slug?: string; href?: string }) => {
      if (state.status !== 'ready' || state.path !== path || state.root !== root) {
        setLinkNotice('The selected document is still loading. Try this link again after it opens.');
        return;
      }
      linkAction.current++;
      assets.close();
      setLinkNotice(null);
      setLinkChoices(null);
      if (event.kind === 'wikilink' && event.target) {
        handleWikiLink(event.target, event.heading ?? null);
      } else if (event.kind === 'anchor' && event.slug !== undefined) {
        const handle = mode === 'split' || mode === 'preview' ? splitPreviewRef.current : readerRef.current;
        if (!handle?.scrollToFragment(event.slug)) setLinkNotice('The requested location is unavailable in this document.');
      } else if (event.kind === 'relative' && event.href && state.dir) {
        if (isAttachmentLink(event.href)) {
          assets.open(event.href);
          return;
        }
        const match = resolveWorkspaceLink(files, state.dir, event.href);
        if (match) {
          if (match.file.path === path && match.fragment) {
            const handle = mode === 'split' || mode === 'preview' ? splitPreviewRef.current : readerRef.current;
            if (!handle?.scrollToFragment(match.fragment)) setLinkNotice('The requested location is unavailable in this document.');
          } else {
            if (match.fragment) pendingHeadingByPath.current.set(match.file.path, match.fragment);
            onNavigate({ path: match.file.path, name: match.file.name });
          }
        } else setLinkNotice('The linked document is unavailable in this open folder.');
      }
    },
    [files, mode, path, root, assets, onNavigate, state.status, state.path, state.root, state.dir, handleWikiLink],
  );

  const platform = useMemo(() => createReaderPlatform(state.dir ?? null, setLinkNotice), [state.dir]);

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
  const showEditor = buffer !== undefined;
  const editorVisible = mode === 'source' || mode === 'split';
  const showReader = mode !== 'source';
  const splitDoc = previewDoc;

  return (
    <div className="reader-region">
      {mode === 'read' ? <ReaderToolbar
        docRef={localRef}
        root={root}
        outlineVisible={outlineVisible}
        onToggleOutline={onToggleOutline}
        hasOutline={outline.length > 0}
        mode={mode}
        onSetMode={onSetMode}
        canEdit={state.status === 'ready' && state.path === path && state.root === root}
        readOnly={readOnly}
      /> : <EditorToolbar title={localRef?.name ?? 'Markdown document'} mode={mode} narrow={narrow} commandsRef={commandsRef}
        dirty={editEntry?.dirty ?? false} saving={localSaving} saveDisabled={localSaving || !editEntry?.dirty}
        saveLabel={localSaving ? 'Saving…' : 'Save'} leaving={exit.busy} onSetMode={onSetMode} onRead={() => void exit.done()}
        onSave={() => void saveActive()} copying={copy.busy} onSaveCopy={() => { if (buffer !== undefined) void copy.save(buffer, localRef?.name ?? 'Document.md'); }} />}
      {mode !== 'read' ? <EditorExitNotice error={exit.error} retry={() => void exit.done()} copying={copy.busy} saveCopy={() => { if (buffer !== undefined) void copy.save(buffer, localRef?.name ?? 'Document.md'); }} /> : editEntry?.dirty ? <p className="editor-draft-kept" role="status">Draft kept · Reading saved version</p> : null}
      {copy.notice && mode !== 'read' ? <p role="status">{copy.notice}</p> : null}
      {copy.error && mode !== 'read' ? <p role="alert">{copy.error}</p> : null}
      {linkNotice ? <p role="status">{linkNotice}</p> : null}
      {linkChoices && linkChoices.path === path && linkChoices.root === root ? <WorkspaceLinkChoices
        key={linkChoices.action} candidates={linkChoices.candidates} root={linkChoices.root}
        canRestoreFocus={() => pathRef.current === linkChoices.path && rootRef.current === linkChoices.root}
        onDismiss={() => { linkAction.current++; setLinkChoices(null); }}
        onChoose={candidate => {
          if (pathRef.current !== linkChoices.path || rootRef.current !== linkChoices.root || linkAction.current !== linkChoices.action) return;
          linkAction.current++;
          if (linkChoices.heading) pendingHeadingByPath.current.set(candidate.path, linkChoices.heading);
          setLinkChoices(null);
          onNavigate(candidate);
        }} /> : null}
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
      <AttachmentLayer origin={currentOrigin} preview={assets.preview} onClose={() => { linkAction.current++; assets.close(); }}>
      <div className={`reader-body reader-body--${mode}`}>
        {showEditor ? (
          <div className="source-region" hidden={!editorVisible}>
            <SourcePane key={`${root}:${tabId}:${path}`} documentKey={`${tabId} ${path}`} visible={editorVisible} commandsRef={commandsRef}
              value={buffer!}
              onChange={onEditBuffer}
              onSave={() => void saveActive()}
              onToggleMode={() => void exit.done()}
              onScrollLine={mode === 'split' ? syncPreviewToLine : undefined}
            />
          </div>
        ) : null}
        {showReader ? (
          <div className="reader-host" style={hostStyle}>
            {mode === 'split' || mode === 'preview' ? (
              splitDoc ? (
                <LocalImageProvider images={assets.images}><PlatformProvider platform={platform}>
                  <MarkdownReader
                    ref={splitPreviewRef}
                    doc={splitDoc}
                    onNavigate={onReaderNavigate}
                    resolveWikiLink={resolveWikiLink}
                    components={{ img: LocalSourceImage }}
                  />
                </PlatformProvider></LocalImageProvider>
              ) : <p role={previewError ? 'alert' : 'status'}>{previewError ?? 'Updating preview…'}</p>
            ) : state.status === 'error' ? (
              <div className="reader-empty">
                <p className="reader-empty__hint">Could not open this document: {state.error}</p>
              </div>
            ) : state.doc ? (
              <LocalImageProvider images={assets.images}><PlatformProvider platform={platform}>
                <MarkdownReader
                  ref={readerRef}
                  doc={state.doc}
                  activeSlug={activeSlug}
                  highlightRange={pulse ?? undefined}
                  onActiveHeadingChange={handleActiveHeading}
                  onNavigate={onReaderNavigate}
                  resolveWikiLink={resolveWikiLink}
                  onScrollAnchorChange={persistScroll}
                  components={{ img: LocalSourceImage }}
                />
              </PlatformProvider></LocalImageProvider>
            ) : null}
          </div>
        ) : null}
        {mode === 'read' && outlineVisible && state.doc ? (
          <OutlineRail items={outline} activeSlug={activeSlug} onSelect={onOutlineSelect} />
        ) : null}
      </div>
      </AttachmentLayer>
    </div>
  );
}
